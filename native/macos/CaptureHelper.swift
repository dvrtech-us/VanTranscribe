// Core Audio ownership and JSON publication live on this control/worker thread.
// The IOProc calls only the preallocated C SPSC ring (no allocation or pipe IO).
import Foundation
import CoreAudio
import AVFoundation
import AppKit
import IOKit.pwr_mgt

enum CaptureFailure: Error, CustomStringConvertible {
    case message(String)
    var description: String { switch self { case .message(let value): return value } }
}

func checked(_ status: OSStatus, _ operation: String) throws {
    if status != noErr { throw CaptureFailure.message("\(operation) failed (Core Audio \(status)). Verify audio permissions and device availability.") }
}

func address(_ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
}

// Capture diagnostics: durations, counts and enumerated names only. They are
// published only by a capture session (never by read-only probes, whose stdout
// must remain one JSON document) as `status` events carrying a typed
// `diagnostic` object, so a parent that predates them ignores them as status.
var diagnosticsEnabled = false
var diagnosticPhase = "idle"
var maxEmitNs: UInt64 = 0
let slowCallThresholdNs: UInt64 = 100_000_000

func diagnosticEvent(_ kind: String, _ fields: [String: Any]) -> [String: Any] {
    var diagnostic = fields
    diagnostic["kind"] = kind
    return ["type": "status", "message": "Capture diagnostic.", "diagnostic": diagnostic]
}

func emitDiagnostic(_ kind: String, _ fields: [String: Any]) {
    if diagnosticsEnabled { emit(diagnosticEvent(kind, fields)) }
}

// Property selectors are public Core Audio constants, not device identity.
func fourCC(_ value: UInt32) -> String {
    let bytes = [24, 16, 8, 0].map { UInt8((value >> UInt32($0)) & 0xff) }
    let readable = bytes.allSatisfy { (0x30...0x39).contains($0) || (0x41...0x5a).contains($0) || (0x61...0x7a).contains($0) || $0 == 0x23 }
    return readable ? String(decoding: bytes, as: UTF8.self) : String(format: "%08x", value)
}

// Reports one Core Audio call that blocked the control thread for more than
// 100 ms. A call that never returns is visible only as missing heartbeats.
func timedCall<T>(_ name: @autoclosure () -> String, _ body: () throws -> T) rethrows -> T {
    let begin = mach_absolute_time()
    defer {
        let spent = vt_elapsed_ns(mach_absolute_time(), begin)
        if spent > slowCallThresholdNs {
            emitDiagnostic("slow_call", ["call": name(), "duration_ns": spent, "phase": diagnosticPhase])
        }
    }
    return try body()
}

func scalar<T: BitwiseCopyable>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ initial: T, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> T {
    var property = address(selector, scope)
    var value = initial
    var size = UInt32(MemoryLayout<T>.size)
    try checked(timedCall("AudioObjectGetPropertyData:\(fourCC(selector))") { AudioObjectGetPropertyData(object, &property, 0, nil, &size, &value) }, "Read audio device property")
    return value
}

func objects(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> [AudioObjectID] {
    var property = address(selector, scope)
    var size: UInt32 = 0
    try checked(timedCall("AudioObjectGetPropertyDataSize:\(fourCC(selector))") { AudioObjectGetPropertyDataSize(object, &property, 0, nil, &size) }, "Read audio object list size")
    if size == 0 { return [] }
    var values = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    try checked(timedCall("AudioObjectGetPropertyData:\(fourCC(selector))") { AudioObjectGetPropertyData(object, &property, 0, nil, &size, &values) }, "Read audio object list")
    return values
}

func stringProperty(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) throws -> String {
    var property = address(selector)
    var value: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    try checked(timedCall("AudioObjectGetPropertyData:\(fourCC(selector))") { AudioObjectGetPropertyData(object, &property, 0, nil, &size, &value) }, "Read audio device name")
    guard let value = value else { throw CaptureFailure.message("The audio device returned no name.") }
    return value.takeRetainedValue() as String
}

// Read-only meeting hints. The catalog is compiled once from the same definition
// Rust validates; no raw PID, process path or unknown bundle ID leaves this code.
struct KnownMeetingApp: Equatable {
    let id: String, name: String, kind: String
    var json: [String: Any] { ["id": id, "name": name, "kind": kind] }
}
let meetingAppCatalog: [KnownMeetingApp] = (0..<vt_meeting_app_count()).map { index in
    KnownMeetingApp(id: String(cString: vt_meeting_app_id(index)!),
                    name: String(cString: vt_meeting_app_name(index)!),
                    kind: String(cString: vt_meeting_app_kind(index)!))
}
let ownBundleID = "com.dvrtech.vantranscribe"
enum MeetingAppResolution: Equatable { case known(KnownMeetingApp), excluded, unknown }

// Responsibility takes precedence over a helper's incidental parent. A bounded
// parent walk remains available when responsibility is unavailable/unrecognized.
func resolveMeetingApp(_ pid: Int32, excluded: Set<Int32>, responsible: (Int32) -> Int32,
                       parent: (Int32) -> Int32, bundle: (Int32) -> String?) -> MeetingAppResolution {
    func identify(_ candidate: Int32) -> MeetingAppResolution {
        if excluded.contains(candidate) { return .excluded }
        guard let id = bundle(candidate) else { return .unknown }
        if id == ownBundleID { return .excluded }
        return meetingAppCatalog.first(where: { $0.id == id }).map { .known($0) } ?? .unknown
    }
    guard pid > 1 else { return .unknown }
    if excluded.contains(pid) { return .excluded }
    let owner = responsible(pid)
    if owner > 1 {
        let result = identify(owner)
        if result != .unknown { return result }
    }
    var current = pid
    var seen = Set<Int32>()
    for _ in 0..<32 {
        guard current > 1, seen.insert(current).inserted else { return .unknown }
        let result = identify(current)
        if result != .unknown { return result }
        current = parent(current)
    }
    return .unknown
}

func meetingScalar<T: BitwiseCopyable>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ initial: T) -> T? {
    var property = address(selector)
    var value = initial
    var size = UInt32(MemoryLayout<T>.size)
    guard AudioObjectGetPropertyData(object, &property, 0, nil, &size, &value) == noErr,
          size == MemoryLayout<T>.size else { return nil }
    return value
}

func meetingProcessObjects() -> [AudioObjectID]? {
    var property = address(kAudioHardwarePropertyProcessObjectList)
    var size: UInt32 = 0
    let system = AudioObjectID(kAudioObjectSystemObject)
    guard AudioObjectGetPropertyDataSize(system, &property, 0, nil, &size) == noErr,
          size % 4 == 0, size <= 4096 * 4 else { return nil }
    if size == 0 { return [] }
    let expected = size
    var values = [AudioObjectID](repeating: 0, count: Int(size) / 4)
    guard AudioObjectGetPropertyData(system, &property, 0, nil, &size, &values) == noErr,
          size == expected, !values.contains(0), Set(values).count == values.count else { return nil }
    return values
}

struct MeetingObservation {
    var complete = true
    var apps: [String: KnownMeetingApp] = [:]
    mutating func include(_ resolution: MeetingAppResolution) {
        switch resolution {
        case .known(let app): apps[app.id] = app
        case .excluded: break
        case .unknown: complete = false
        }
    }
    var json: [String: Any] {
        ["complete": complete, "apps": apps.values.sorted { $0.id < $1.id }.map { $0.json },
         "error": complete ? NSNull() : "Some microphone activity could not be attributed. Meeting-app status is unknown." as Any]
    }
}

func observeMeetingApps() -> MeetingObservation {
    var observation = MeetingObservation()
    guard let processes = meetingProcessObjects() else { observation.complete = false; return observation }
    let excluded: Set<Int32> = [getpid(), getppid()]
    for process in processes {
        guard let running = meetingScalar(process, kAudioProcessPropertyIsRunningInput, UInt32(0)), running <= 1 else {
            observation.complete = false; continue
        }
        if running == 0 { continue }
        guard let pid = meetingScalar(process, kAudioProcessPropertyPID, Int32(0)), pid > 1 else {
            observation.complete = false; continue
        }
        observation.include(resolveMeetingApp(pid, excluded: excluded,
            responsible: { vt_responsible_pid($0) }, parent: { vt_parent_pid($0) },
            bundle: { NSRunningApplication(processIdentifier: $0)?.bundleIdentifier }))
    }
    // A changing/failed process inventory is not a complete absence observation.
    if meetingProcessObjects().map({ Set($0) == Set(processes) }) != true { observation.complete = false }
    return observation
}

func testMeetingApps() {
    let zoom = meetingAppCatalog.first { $0.id == "us.zoom.xos" }!
    let chrome = meetingAppCatalog.first { $0.id == "com.google.Chrome" }!
    precondition(meetingAppCatalog.count == Set(meetingAppCatalog.map { $0.id }).count)
    precondition(meetingAppCatalog.allSatisfy { ["meeting", "browser"].contains($0.kind) })
    func resolve(_ owner: Int32 = -1, _ parents: [Int32: Int32] = [:], _ bundles: [Int32: String] = [:], _ excluded: Set<Int32> = []) -> MeetingAppResolution {
        resolveMeetingApp(10, excluded: excluded, responsible: { _ in owner }, parent: { parents[$0] ?? -1 }, bundle: { bundles[$0] })
    }
    precondition(resolve(20, [10: 30], [20: zoom.id, 30: chrome.id]) == .known(zoom))
    precondition(resolve(-1, [10: 20], [20: chrome.id]) == .known(chrome))
    precondition(resolve(20, [10: 30], [20: ownBundleID, 30: chrome.id]) == .excluded)
    precondition(resolve(-1, [10: 20], [20: ownBundleID]) == .excluded)
    precondition(resolve(20, [:], [20: zoom.id], [20]) == .excluded)
    precondition(resolve(-1, [10: 20, 20: 10]) == .unknown)
    precondition(resolve(-1, [:], [10: "com.google.Chrome.helper"]) == .unknown)
    precondition(resolve(-1, [10: 1], [10: "com.unknown.meeting"]) == .unknown)
    var walkCalls = 0
    precondition(resolveMeetingApp(10, excluded: [], responsible: { _ in -1 }, parent: { walkCalls += 1; return $0 + 1 }, bundle: { _ in nil }) == .unknown)
    precondition(walkCalls == 32)
    var observation = MeetingObservation()
    observation.include(.known(zoom)); observation.include(.known(zoom)); observation.include(.excluded)
    precondition(observation.complete && observation.apps.count == 1)
    observation.include(.unknown)
    precondition(!observation.complete && observation.apps.count == 1)
    emit(["type": "status", "message": "Meeting-app tests passed: exact catalog, responsibility priority, bounded ancestry, self exclusion, unknown/loop handling and normalized deduplication."])
}

func streamChannels(_ device: AudioObjectID, _ scope: AudioObjectPropertyScope) throws -> UInt32 {
    var property = address(kAudioDevicePropertyStreamConfiguration, scope)
    var size: UInt32 = 0
    try checked(timedCall("AudioObjectGetPropertyDataSize:slay") { AudioObjectGetPropertyDataSize(device, &property, 0, nil, &size) }, "Read input stream configuration")
    guard size >= MemoryLayout<UInt32>.size else { return 0 }
    let storage = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
    defer { storage.deallocate() }
    try checked(timedCall("AudioObjectGetPropertyData:slay") { AudioObjectGetPropertyData(device, &property, 0, nil, &size, storage) }, "Read input channels")
    return UnsafeMutableAudioBufferListPointer(storage.assumingMemoryBound(to: AudioBufferList.self)).reduce(0) { $0 + $1.mNumberChannels }
}

func inputChannels(_ device: AudioObjectID) throws -> UInt32 {
    try streamChannels(device, kAudioDevicePropertyScopeInput)
}

// Owner-approved per-epoch evidence: transport class only, never name or UID.
func inputTransport(_ value: UInt32?) -> String {
    let transport = outputTransport(value)
    return ["built_in", "bluetooth", "usb", "aggregate", "virtual", "unknown"].contains(transport) ? transport : "other"
}

func isBluetooth(_ device: AudioObjectID) -> Bool {
    let kind = (try? scalar(device, kAudioDevicePropertyTransportType, UInt32(0))) ?? 0
    return kind == kAudioDeviceTransportTypeBluetooth || kind == kAudioDeviceTransportTypeBluetoothLE
}

let originTicks = mach_absolute_time()
func elapsedNow() -> UInt64 { vt_elapsed_ns(mach_absolute_time(), originTicks) }

func emit(_ value: [String: Any]) {
    do {
        var bytes = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
        bytes.append(10)
        // Pipe backpressure blocks this control thread; heartbeats report the
        // longest single write so a slow consumer is distinguishable.
        let begin = mach_absolute_time()
        try FileHandle.standardOutput.write(contentsOf: bytes)
        maxEmitNs = max(maxEmitNs, vt_elapsed_ns(mach_absolute_time(), begin))
    } catch { vt_request_stop() }
}

func emitGap(epoch: UInt32, start: UInt64, end: UInt64, reason: String, mic: Bool, system: Bool, overflow: OverflowGap? = nil) {
    guard end > start else { return }
    for track in [mic ? "mic" : nil, system ? "system" : nil].compactMap({ $0 }) {
        var event: [String: Any] = ["type": "gap", "track": track, "epoch": epoch, "start_ns": start, "end_ns": end, "reason": reason]
        if let overflow = overflow {
            // Exact frame accounting for a coalesced overflow burst (see OverflowCoalescer).
            event["dropped_frames"] = overflow.droppedFrames
            event["discarded_frames"] = overflow.discardedFrames
            event["overflow_runs"] = overflow.runs
        }
        emit(event)
    }
}

// Overflow runs separated by less than one second of delivered audio are
// published as one gap from the first dropped frame to the last dropped
// frame. The delivered audio between runs is discarded and counted, so
// gap duration = dropped frames + discarded frames (host-timestamp range).
// During ring saturation each freed slot admits roughly one IO quantum
// (~10 ms) between drops, so the extra discarded audio is small, while one
// gap replaces up to dozens of live-inference resets. A run followed by a
// full second of clean audio is closed; the held audio is then released.
struct OverflowGap: Equatable {
    let epoch: UInt32
    var start: UInt64
    var end: UInt64
    var droppedFrames: UInt64
    var discardedFrames: UInt64
    var runs: UInt32
}

struct OverflowCoalescer<Packet> {
    enum Output { case gap(OverflowGap), packet(Packet) }
    static var windowNs: UInt64 { 1_000_000_000 }
    let maxHeldFrames: UInt64
    private(set) var pending: OverflowGap?
    private var held: [(packet: Packet, frames: UInt64)] = []
    private var heldFrames: UInt64 = 0
    init(maxHeldFrames: UInt64) { self.maxHeldFrames = maxHeldFrames }

    mutating func drop(epoch: UInt32, start: UInt64, end: UInt64, frames: UInt64) -> [Output] {
        if var gap = pending, gap.epoch == epoch, start < gap.end || start - gap.end < Self.windowNs {
            gap.discardedFrames += heldFrames
            held.removeAll(); heldFrames = 0
            gap.end = max(gap.end, end)
            gap.droppedFrames += frames
            gap.runs += 1
            pending = gap
            return []
        }
        let closed = finish()
        pending = OverflowGap(epoch: epoch, start: start, end: max(start, end), droppedFrames: frames, discardedFrames: 0, runs: 1)
        return closed
    }

    mutating func packet(_ packet: Packet, end: UInt64, frames: UInt64) -> [Output] {
        guard let gap = pending else { return [.packet(packet)] }
        held.append((packet, frames)); heldFrames += frames
        // The frame bound keeps memory finite even if host timestamps stall.
        if (end >= gap.end && end - gap.end >= Self.windowNs) || heldFrames >= maxHeldFrames { return finish() }
        return []
    }

    mutating func finish() -> [Output] {
        guard let gap = pending else { return [] }
        let result = [Output.gap(gap)] + held.map { Output.packet($0.packet) }
        pending = nil; held.removeAll(); heldFrames = 0
        return result
    }
}

func listDevices() throws {
    let defaultInput = try scalar(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice, AudioObjectID(0))
    let devices = try objects(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDevices)
    var result: [[String: Any]] = []
    for device in devices where ((try? inputChannels(device)) ?? 0) > 0 {
        result.append(["id": try stringProperty(device, kAudioDevicePropertyDeviceUID),
                       "name": try stringProperty(device, kAudioObjectPropertyName),
                       "is_default": device == defaultInput, "bluetooth": isBluetooth(device)])
    }
    emit(["helper_protocol": 4, "supported_commands": supportedCommands, "capture_capabilities": captureCapabilities, "devices": result, "system_supported": ProcessInfo.processInfo.isOperatingSystemAtLeast(OperatingSystemVersion(majorVersion: 14, minorVersion: 4, patchVersion: 0))])
}

func inputIdentity(uid: String?) throws -> AudioObjectID {
    if let uid = uid {
        for device in try objects(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDevices) {
            if (try? stringProperty(device, kAudioDevicePropertyDeviceUID)) == uid { return device }
        }
        throw CaptureFailure.message("The selected microphone is no longer available. Select another input device.")
    }
    let device = try scalar(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice, AudioObjectID(0))
    guard device != kAudioObjectUnknown else { throw CaptureFailure.message("No default microphone is available.") }
    return device
}

func selectedInput(uid: String?) throws -> AudioObjectID {
    let device = try inputIdentity(uid: uid)
    guard uid != nil || !isBluetooth(device) else {
        throw CaptureFailure.message("The default microphone is Bluetooth. Select a built-in/USB microphone, or explicitly select this Bluetooth input to allow headset call mode.")
    }
    return device
}

// Core Audio can notify observers even when a property's value did not change.
// Compare only capture-relevant values, never volatile object/stream identities.
func rateFingerprint(_ rate: Double) -> Int64? {
    guard rate.isFinite, rate >= 0, rate <= 1_000_000 else { return nil }
    return Int64((rate * 1_000).rounded())
}

struct StreamFingerprint: Equatable {
    let rate: Int64?
    let format: UInt32
    let flags: UInt32
    let bytesPerPacket: UInt32
    let framesPerPacket: UInt32
    let bytesPerFrame: UInt32
    let channels: UInt32
    let bits: UInt32
    init(_ value: AudioStreamBasicDescription) {
        rate = rateFingerprint(value.mSampleRate)
        format = value.mFormatID; flags = value.mFormatFlags
        bytesPerPacket = value.mBytesPerPacket; framesPerPacket = value.mFramesPerPacket
        bytesPerFrame = value.mBytesPerFrame; channels = value.mChannelsPerFrame
        bits = value.mBitsPerChannel
    }
}

struct CaptureFingerprint: Equatable {
    var input: AudioObjectID?
    var inputAlive: Bool?
    var inputRate: Int64?
    var inputChannels: UInt32?
    var aggregateChannels: UInt32?
    var aggregateFormat: StreamFingerprint?
    var streams: [StreamFingerprint]?
    var quantum: UInt32?
    var output: AudioObjectID?
    var outputRate: Int64?
    var outputChannels: UInt32?
    var exclusions: [AudioObjectID]

    // A failed query is not evidence: keep the last successfully read value.
    func merging(_ newer: CaptureFingerprint) -> CaptureFingerprint {
        CaptureFingerprint(
            input: newer.input ?? input, inputAlive: newer.inputAlive ?? inputAlive,
            inputRate: newer.inputRate ?? inputRate, inputChannels: newer.inputChannels ?? inputChannels,
            aggregateChannels: newer.aggregateChannels ?? aggregateChannels,
            aggregateFormat: newer.aggregateFormat ?? aggregateFormat, streams: newer.streams ?? streams,
            quantum: newer.quantum ?? quantum, output: newer.output ?? output,
            outputRate: newer.outputRate ?? outputRate, outputChannels: newer.outputChannels ?? outputChannels,
            exclusions: exclusions)
    }
}

// What the running ring was sized and labeled for. A change to any of these
// means the running stream is wrong and must be rebuilt even while settling.
struct OpenedStream: Equatable {
    let input: AudioObjectID
    let channels: UInt32
    let format: StreamFingerprint
    let streams: [StreamFingerprint]
    let quantum: UInt32

    func invalidated(by current: CaptureFingerprint) -> Bool {
        if let value = current.input, value != input { return true }
        if let value = current.aggregateChannels, value != channels { return true }
        if let value = current.aggregateFormat, value != format { return true }
        if let value = current.streams, value != streams { return true }
        // A smaller IO quantum fits the ring; a larger one would drop every callback.
        if let value = current.quantum, value > quantum { return true }
        return false
    }
}

struct ConfigurationChangeGate {
    private(set) var baseline: CaptureFingerprint
    var candidate: CaptureFingerprint?
    var candidateSince: UInt64 = 0
    let opened: OpenedStream?
    private(set) var settleStartedAt: UInt64?
    private(set) var provisionalSince: UInt64 = 0
    private(set) var absorbed: UInt32 = 0
    private(set) var settledStable = false
    static let stabilityWindowNs: UInt64 = 75_000_000
    // Opening an input can itself change the route (for example a Bluetooth
    // headset switching to its call profile, or the aggregate/output format
    // renegotiating). Those consequences of our own start are absorbed into the
    // baseline until it has been unchanged for the stability window and at least
    // one second has passed; a stream that never settles is armed after ten.
    static let settleMinimumNs: UInt64 = 1_000_000_000
    static let settleMaximumNs: UInt64 = 10_000_000_000
    static let settleCheckIntervalNs: UInt64 = 250_000_000

    init(baseline: CaptureFingerprint) { self.baseline = baseline; opened = nil }
    init(settling provisional: CaptureFingerprint, opened: OpenedStream, at now: UInt64) {
        baseline = provisional; self.opened = opened
        settleStartedAt = now; provisionalSince = now
    }

    var settling: Bool { settleStartedAt != nil }

    mutating func observe(_ current: CaptureFingerprint, at now: UInt64) -> String? {
        if let started = settleStartedAt, let opened = opened {
            if current.inputAlive == false { candidate = nil; return "device_change" }
            // A real change to the opened device or ring format is never absorbed.
            if opened.invalidated(by: current) { return confirm(current, at: now, reason: "device_change") }
            candidate = nil
            // Published exclusions stay in the baseline, so an exclusion change
            // seen while settling still rebuilds once the gate is armed.
            let adopted = baseline.merging(current)
            if adopted != baseline { baseline = adopted; provisionalSince = now; absorbed += 1 }
            let elapsed = now >= started ? now - started : 0
            let stable = now >= provisionalSince && now - provisionalSince >= Self.stabilityWindowNs
            if (elapsed >= Self.settleMinimumNs && stable) || elapsed >= Self.settleMaximumNs {
                settleStartedAt = nil
                settledStable = stable
            }
            return nil
        }
        if current == baseline { candidate = nil; return nil }
        // Do not debounce a confirmed dead selected microphone. A failed
        // identity/property query alone is uncertain, so it gets the same short
        // stability window rather than causing an immediate rebuild cascade.
        if current.inputAlive == false {
            candidate = nil
            return "device_change"
        }
        return confirm(current, at: now, reason: current.exclusions != baseline.exclusions ? "process_exclusion_change" : "device_change")
    }

    private mutating func confirm(_ current: CaptureFingerprint, at now: UInt64, reason: String) -> String? {
        if candidate != current {
            candidate = current
            candidateSince = now
            return nil
        }
        guard now >= candidateSince, now - candidateSince >= Self.stabilityWindowNs else { return nil }
        candidate = nil
        return reason
    }
}

func microphonePermission() throws {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return
    case .notDetermined:
        let done = DispatchSemaphore(value: 0)
        var allowed = false
        AVCaptureDevice.requestAccess(for: .audio) { granted in allowed = granted; done.signal() }
        while done.wait(timeout: .now() + 0.05) == .timedOut {
            if vt_stop_requested() { throw CaptureFailure.message("Recording was cancelled while waiting for microphone permission.") }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
        }
        guard allowed else { throw CaptureFailure.message("Microphone access was not granted. Enable VanTranscribe in System Settings → Privacy & Security → Microphone, then retry.") }
    default:
        throw CaptureFailure.message("Microphone access is unavailable. Enable VanTranscribe in System Settings → Privacy & Security → Microphone, then retry.")
    }
}

// Only public process-parent inspection is used. Never exclude every WebKit
// process globally: Safari could itself be the meeting source.
func excludedAudioProcesses(parentPID: Int32) -> [AudioObjectID] {
    let all = (try? objects(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyProcessObjectList)) ?? []
    return all.filter { object in
        guard var pid = try? scalar(object, kAudioProcessPropertyPID, Int32(0)), pid > 0 else { return false }
        // This helper has no render path. Excluding its capture-only process
        // object would cause needless rebuilds as its aggregate is recreated.
        if pid == getpid() { return false }
        for _ in 0..<64 {
            if pid == parentPID { return true }
            if pid <= 1 { break }
            let next = vt_parent_pid(pid)
            if next == pid || next <= 0 { break }
            pid = next
        }
        return false
    }.sorted()
}

// Route evidence is collected only on the control thread. Built-in transport
// alone includes headphone outputs and never qualifies as speaker evidence.
struct OutputRoute: Codable, Equatable {
    var kind = "unknown"
    var transport = "unknown"
    var default_output_known = false
    var data_source_known = false
    var speaker_terminal = false
    var headphone_jack_connected: Bool? = nil
    var active_output_processes: UInt32 = 0
    var process_devices_known = false
    var all_processes_same_output = false
    var global_tap = false
    var reference_available = false
    var reason = "unknown"

    mutating func classify() {
        if headphone_jack_connected == true || kind == "headphones" { kind = "headphones" }
        else if transport != "built_in" && transport != "unknown" { kind = "external" }
        else if transport == "built_in" && default_output_known && data_source_known && speaker_terminal && headphone_jack_connected == false { kind = "built_in_speakers" }
        else { kind = "unknown" }
        if !global_tap || !reference_available { reason = "no_reference" }
        else if kind == "headphones" { reason = "headphones" }
        else if kind == "external" { reason = "external_output" }
        else if kind != "built_in_speakers" { reason = "output_evidence_unavailable" }
        else if !process_devices_known { reason = "process_evidence_unavailable" }
        else if active_output_processes == 0 { reason = "no_active_output" }
        else if !all_processes_same_output { reason = "mixed_process_outputs" }
        else { reason = "eligible" }
    }

    var json: [String: Any] {
        // Codable deliberately emits no identifiers, process names or device names.
        (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(self))) as? [String: Any] ?? [:]
    }
}

func outputTransport(_ value: UInt32?) -> String {
    guard let value = value else { return "unknown" }
    switch value {
    case kAudioDeviceTransportTypeBuiltIn: return "built_in"
    case kAudioDeviceTransportTypeUSB: return "usb"
    case kAudioDeviceTransportTypeBluetooth, kAudioDeviceTransportTypeBluetoothLE: return "bluetooth"
    case kAudioDeviceTransportTypeHDMI: return "hdmi"
    case kAudioDeviceTransportTypeDisplayPort: return "display_port"
    case kAudioDeviceTransportTypeAggregate: return "aggregate"
    case kAudioDeviceTransportTypeVirtual: return "virtual"
    case 0: return "unknown"
    default: return "other"
    }
}

struct ProcessOutputRoute: Equatable {
    let process: AudioObjectID
    let running: UInt32?
    let devices: [AudioObjectID]?
}

struct OutputRouteObservation: Equatable {
    let output: AudioObjectID?
    let sources: [UInt32]?
    let terminals: [UInt32]?
    let processes: [ProcessOutputRoute]?
    let route: OutputRoute
}

func applyProcessEvidence(_ processes: [ProcessOutputRoute]?, output: AudioObjectID?, route: inout OutputRoute) {
    route.process_devices_known = processes.map { $0.allSatisfy { item in
        item.running == 0 || (item.running == 1 && item.devices?.isEmpty == false)
    } } ?? false
    route.active_output_processes = UInt32(processes?.filter { $0.running == 1 }.count ?? 0)
    route.all_processes_same_output = route.process_devices_known && route.active_output_processes > 0 && processes!.filter { $0.running == 1 }.allSatisfy { $0.devices == output.map { [$0] } }
}

func observeOutputRoute(exclusions: [AudioObjectID], referenceAvailable: Bool) -> OutputRouteObservation {
    let output = (try? scalar(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice, AudioObjectID(0))).flatMap { $0 == kAudioObjectUnknown ? nil : $0 }
    var route = OutputRoute()
    route.default_output_known = output != nil
    route.global_tap = referenceAvailable
    route.reference_available = referenceAvailable
    route.transport = outputTransport(output.flatMap { try? scalar($0, kAudioDevicePropertyTransportType, UInt32(0)) })
    let sources = output.flatMap { try? objects($0, kAudioDevicePropertyDataSource, kAudioDevicePropertyScopeOutput).sorted() }
    route.data_source_known = sources.map { !$0.isEmpty } ?? false
    route.headphone_jack_connected = output.flatMap { try? scalar($0, kAudioDevicePropertyJackIsConnected, UInt32(0), kAudioDevicePropertyScopeOutput) }.flatMap { $0 <= 1 ? $0 != 0 : nil }
    let terminals: [UInt32]? = output.flatMap { device in
        guard let streams = try? objects(device, kAudioDevicePropertyStreams, kAudioDevicePropertyScopeOutput), !streams.isEmpty else { return nil }
        var types: [UInt32] = []
        for stream in streams {
            guard let active = try? scalar(stream, kAudioStreamPropertyIsActive, UInt32(0)), active <= 1 else { return nil }
            if active == 0 { continue }
            guard let terminal = try? scalar(stream, kAudioStreamPropertyTerminalType, UInt32(0)) else { return nil }
            types.append(terminal)
        }
        return types.sorted()
    }
    route.speaker_terminal = terminals.map { !$0.isEmpty && $0.allSatisfy { $0 == kAudioStreamTerminalTypeSpeaker } } ?? false
    if terminals?.contains(kAudioStreamTerminalTypeHeadphones) == true { route.kind = "headphones" }
    let processIDs = try? objects(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyProcessObjectList)
    var processes: [ProcessOutputRoute]? = nil
    if let processIDs = processIDs, processIDs.count <= 4096 {
        processes = processIDs.filter { !exclusions.contains($0) }.sorted().compactMap { process in
            // The helper is capture-only; its private aggregate must not count as
            // another output route or churn the fingerprint on every rebuild.
            if (try? scalar(process, kAudioProcessPropertyPID, Int32(0))) == getpid() { return nil }
            let running = try? scalar(process, kAudioProcessPropertyIsRunningOutput, UInt32(0))
            let devices = running == 1 ? try? objects(process, kAudioProcessPropertyDevices, kAudioDevicePropertyScopeOutput).sorted() : nil
            return ProcessOutputRoute(process: process, running: running, devices: devices)
        }
    }
    applyProcessEvidence(processes, output: output, route: &route)
    route.classify()
    return OutputRouteObservation(output: output, sources: sources, terminals: terminals, processes: processes, route: route)
}

let captureIOProc: AudioDeviceIOProc = { _, _, input, inputTime, _, _, client in
    vt_count_callback()
    guard let client = client else { return noErr }
    guard inputTime.pointee.mFlags.contains(.hostTimeValid) else { return noErr }
    vt_ring_write(OpaquePointer(client), input, inputTime.pointee.mHostTime)
    return noErr
}

final class Accumulator {
    let track: String
    let channels: UInt32
    let rate: UInt32
    var epoch: UInt32 = 0
    var first: UInt64 = 0
    var host: UInt64 = 0
    var samples: [Float] = []
    init(track: String, channels: UInt32, rate: UInt32) {
        self.track = track; self.channels = channels; self.rate = rate
        samples.reserveCapacity(Int(rate / 5 * channels))
    }
    func append(_ incoming: [Float], _ info: VTFrameInfo) {
        if !samples.isEmpty && (epoch != info.epoch || first + UInt64(samples.count) / UInt64(channels) != info.first_frame) { flush() }
        if samples.isEmpty { epoch = info.epoch; first = info.first_frame; host = info.host_time_ns }
        samples.append(contentsOf: incoming)
        if samples.count >= Int(rate / 10 * channels) { flush() }
    }
    func flush() {
        if samples.isEmpty { return }
        var power: Double = 0
        for i in samples.indices {
            if !samples[i].isFinite { samples[i] = 0 }
            power += Double(samples[i]) * Double(samples[i])
        }
        emit(["type": "audio", "track": track, "epoch": epoch, "first_frame": first, "host_time_ns": host, "samples": samples])
        emit(["type": "level", "track": track, "value": min(1, sqrt(power / Double(samples.count)) * 3)])
        samples.removeAll(keepingCapacity: true)
    }
}

struct CapturedPacket {
    let info: VTFrameInfo
    let tracks: [[Float]]
}

struct RebuildTrigger {
    let source: String
    let observedAt: UInt64
    static func from(sources: UInt32, firstTicks: UInt64, now: UInt64) -> RebuildTrigger {
        let source = sources & UInt32(VT_REBUILD_WAKE) != 0 ? "wake"
            : sources & UInt32(VT_REBUILD_NOTIFICATION) != 0 ? "notification"
            : sources & UInt32(VT_REBUILD_POLL) != 0 ? "poll" : "unknown"
        let observed = firstTicks == 0 ? now : min(now, vt_elapsed_ns(firstTicks, originTicks))
        return RebuildTrigger(source: source, observedAt: observed)
    }
}

final class CaptureEngine {
    let parentPID: Int32
    let micUID: String?
    let remote: Bool
    var epoch: UInt32 = 0
    var tap: AudioObjectID = kAudioObjectUnknown
    var aggregate: AudioObjectID = kAudioObjectUnknown
    var ioProc: AudioDeviceIOProcID?
    var ring: OpaquePointer?
    var mic: Accumulator?
    var system: Accumulator?
    var listeners: [(AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertyListenerBlock)] = []
    var exclusions: [AudioObjectID] = []
    var configurationGate: ConfigurationChangeGate?
    var outputObservation: OutputRouteObservation?
    var routeListeners: [(AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertyListenerBlock)] = []
    var lastEnd: UInt64 = 0
    var started = false
    var suspended = false
    var overflow = OverflowCoalescer<CapturedPacket>(maxHeldFrames: 0)
    var trigger: RebuildTrigger?
    var lastFingerprintNs: UInt64 = 0
    var nextSettleCheck: UInt64 = 0
    init(parentPID: Int32, micUID: String?, remote: Bool) { self.parentPID = parentPID; self.micUID = micUID; self.remote = remote }
    deinit { stop() }

    func watch(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) {
        var property = address(selector, scope)
        // A notification requests a value check; it does not prove a route or
        // format change, and must not rebuild the aggregate by itself.
        let block: AudioObjectPropertyListenerBlock = { _, _ in vt_request_rebuild() }
        if timedCall("AudioObjectAddPropertyListenerBlock:\(fourCC(selector))", { AudioObjectAddPropertyListenerBlock(object, &property, DispatchQueue.global(qos: .userInitiated), block) }) == noErr { listeners.append((object, property, block)) }
    }

    func clearRouteListeners() {
        for (object, var property, block) in routeListeners { _ = timedCall("AudioObjectRemovePropertyListenerBlock:\(fourCC(property.mSelector))") { AudioObjectRemovePropertyListenerBlock(object, &property, DispatchQueue.global(qos: .userInitiated), block) } }
        routeListeners.removeAll()
    }

    func refreshOutputRoute(force: Bool = false) {
        let current = observeOutputRoute(exclusions: exclusions, referenceAvailable: system != nil && tap != kAudioObjectUnknown)
        if !force && current == outputObservation { return }
        let previous = outputObservation
        outputObservation = current
        emit(["type": "output_route", "epoch": epoch, "start_ns": elapsedNow(), "route": current.route.json])
        // Observer callbacks only request a control-thread comparison. Polling
        // still covers missed process-running/device notifications.
        if previous?.output != current.output || previous?.terminals != current.terminals || previous?.processes?.map({ $0.process }) != current.processes?.map({ $0.process }) || force {
            clearRouteListeners()
            func routeWatch(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) {
                var property = address(selector, scope)
                let block: AudioObjectPropertyListenerBlock = { _, _ in vt_request_rebuild() }
                if timedCall("AudioObjectAddPropertyListenerBlock:\(fourCC(selector))", { AudioObjectAddPropertyListenerBlock(object, &property, DispatchQueue.global(qos: .userInitiated), block) }) == noErr { routeListeners.append((object, property, block)) }
            }
            routeWatch(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyProcessObjectList)
            if let output = current.output {
                routeWatch(output, kAudioDevicePropertyDataSource, kAudioDevicePropertyScopeOutput)
                routeWatch(output, kAudioDevicePropertyJackIsConnected, kAudioDevicePropertyScopeOutput)
                routeWatch(output, kAudioDevicePropertyStreams, kAudioDevicePropertyScopeOutput)
                for stream in (try? objects(output, kAudioDevicePropertyStreams, kAudioDevicePropertyScopeOutput)) ?? [] {
                    routeWatch(stream, kAudioStreamPropertyTerminalType)
                    routeWatch(stream, kAudioStreamPropertyIsActive)
                }
            }
            for item in current.processes ?? [] {
                routeWatch(item.process, kAudioProcessPropertyIsRunningOutput)
                routeWatch(item.process, kAudioProcessPropertyDevices, kAudioDevicePropertyScopeOutput)
            }
        }
    }

    func fingerprint(publishedExclusions: Bool = false) -> CaptureFingerprint {
        let input = try? inputIdentity(uid: micUID)
        let alive = input.flatMap { try? scalar($0, kAudioDevicePropertyDeviceIsAlive, UInt32(0)) }.map { $0 != 0 }
        let inputRate = input.flatMap { try? scalar($0, kAudioDevicePropertyNominalSampleRate, Float64(0)) }.flatMap(rateFingerprint)
        let channels = input.flatMap { try? inputChannels($0) }
        let output = try? scalar(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice, AudioObjectID(0))
        let outputRate = output.flatMap { try? scalar($0, kAudioDevicePropertyNominalSampleRate, Float64(0)) }.flatMap(rateFingerprint)
        let outputChannels = output.flatMap { try? streamChannels($0, kAudioDevicePropertyScopeOutput) }
        let format = (try? scalar(aggregate, kAudioDevicePropertyStreamFormat, AudioStreamBasicDescription(), kAudioDevicePropertyScopeInput)).map(StreamFingerprint.init)
        let streamIDs = try? objects(aggregate, kAudioDevicePropertyStreams, kAudioDevicePropertyScopeInput)
        let streamFormats = streamIDs.flatMap { streams -> [StreamFingerprint]? in
            var result: [StreamFingerprint] = []
            for stream in streams {
                guard let value = try? scalar(stream, kAudioStreamPropertyVirtualFormat, AudioStreamBasicDescription()) else { return nil }
                result.append(StreamFingerprint(value))
            }
            return result
        }
        return CaptureFingerprint(
            input: input, inputAlive: alive, inputRate: inputRate, inputChannels: channels,
            aggregateChannels: try? inputChannels(aggregate), aggregateFormat: format,
            streams: streamFormats, quantum: try? scalar(aggregate, kAudioDevicePropertyBufferFrameSize, UInt32(0)),
            output: output, outputRate: outputRate, outputChannels: outputChannels,
            exclusions: publishedExclusions ? exclusions : (remote ? excludedAudioProcesses(parentPID: parentPID) : []))
    }

    var hasPendingConfigurationChange: Bool { configurationGate?.candidate != nil }

    // While the new stream settles the loop observes on its own cadence, because
    // a consequence of our own start may arrive without a usable notification.
    func settleCheckDue(_ now: UInt64) -> Bool {
        guard configurationGate?.settling == true, now >= nextSettleCheck else { return false }
        nextSettleCheck = now + ConfigurationChangeGate.settleCheckIntervalNs
        return true
    }

    func noteTrigger(_ value: RebuildTrigger) { if trigger == nil { trigger = value } }

    // A trigger whose observation found no change is forgotten, so a rebuild's
    // trigger age measures from the first observation of the change itself.
    func forgetIdleTrigger() { if !hasPendingConfigurationChange { trigger = nil; lastFingerprintNs = 0 } }

    func configurationChangeReason() -> String? {
        let previousPhase = diagnosticPhase
        diagnosticPhase = "fingerprint"
        defer { diagnosticPhase = previousPhase }
        let begin = elapsedNow()
        let current = fingerprint()
        let now = elapsedNow()
        lastFingerprintNs = now - begin
        let wasSettling = configurationGate?.settling == true
        let reason = configurationGate?.observe(current, at: now)
        if wasSettling, let gate = configurationGate, !gate.settling {
            emitDiagnostic("settled", ["epoch": epoch, "settle_ns": now - streamStartedAt, "absorbed": gate.absorbed, "stable": gate.settledStable])
        }
        return reason
    }

    var streamStartedAt: UInt64 = 0

    func start() throws {
        do {
            let input = try selectedInput(uid: micUID)
            let micChannels = try inputChannels(input)
            guard (1...8).contains(micChannels) else { throw CaptureFailure.message("This microphone exposes an unsupported channel count (\(micChannels)). Select an input with 1–8 channels.") }
            let uid = try stringProperty(input, kAudioDevicePropertyDeviceUID)
            var systemChannels: UInt32 = 0
            var description: [String: Any] = [
                kAudioAggregateDeviceNameKey: "VanTranscribe Capture",
                kAudioAggregateDeviceUIDKey: "com.vantranscribe.capture.\(UUID().uuidString)",
                kAudioAggregateDeviceIsPrivateKey: true,
                kAudioAggregateDeviceIsStackedKey: false,
                kAudioAggregateDeviceMainSubDeviceKey: uid,
                kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: uid, kAudioSubDeviceDriftCompensationKey: false]]
            ]
            if remote {
                exclusions = excludedAudioProcesses(parentPID: parentPID)
                let tapDescription = CATapDescription(stereoGlobalTapButExcludeProcesses: exclusions)
                tapDescription.name = "VanTranscribe System Audio"
                tapDescription.isPrivate = true
                tapDescription.muteBehavior = .unmuted
                let status = timedCall("AudioHardwareCreateProcessTap") { AudioHardwareCreateProcessTap(tapDescription, &tap) }
                if status == noErr {
                    systemChannels = 2
                    description[kAudioAggregateDeviceTapListKey] = [[kAudioSubTapUIDKey: tapDescription.uuid.uuidString, kAudioSubTapDriftCompensationKey: true]]
                    // Do not wait for a tapped app to start rendering: the mic
                    // must record even when the remote participants are silent.
                    description[kAudioAggregateDeviceTapAutoStartKey] = false
                } else {
                    emit(["type": "error", "fatal": false, "message": "System audio is unavailable (Core Audio \(status)); recording the microphone only. Verify System Audio Recording permission in System Settings."])
                }
            }
            try checked(timedCall("AudioHardwareCreateAggregateDevice") { AudioHardwareCreateAggregateDevice(description as CFDictionary, &aggregate) }, "Create shared-clock microphone/system aggregate")
            let actualChannels = try inputChannels(aggregate)
            guard actualChannels == micChannels + systemChannels else {
                throw CaptureFailure.message("The aggregate input channel layout is unexpected (\(actualChannels), expected \(micChannels + systemChannels)). Recording stopped to avoid mislabeling the two tracks.")
            }
            let format = try scalar(aggregate, kAudioDevicePropertyStreamFormat, AudioStreamBasicDescription(), kAudioDevicePropertyScopeInput)
            guard format.mFormatID == kAudioFormatLinearPCM, format.mFormatFlags & kAudioFormatFlagIsFloat != 0, format.mBitsPerChannel == 32 else {
                throw CaptureFailure.message("The capture device did not expose 32-bit floating-point PCM.")
            }
            guard format.mSampleRate.isFinite, (8_000...192_000).contains(format.mSampleRate), format.mFormatFlags & kAudioFormatFlagIsBigEndian == 0 else {
                throw CaptureFailure.message("The capture device exposes an unsupported sample rate or byte order.")
            }
            let rate = UInt32(format.mSampleRate.rounded())
            var openedStreams: [StreamFingerprint] = []
            for stream in try objects(aggregate, kAudioDevicePropertyStreams, kAudioDevicePropertyScopeInput) {
                let streamFormat = try scalar(stream, kAudioStreamPropertyVirtualFormat, AudioStreamBasicDescription())
                openedStreams.append(StreamFingerprint(streamFormat))
                guard streamFormat.mFormatID == kAudioFormatLinearPCM,
                      streamFormat.mFormatFlags & kAudioFormatFlagIsFloat != 0,
                      streamFormat.mFormatFlags & kAudioFormatFlagIsBigEndian == 0,
                      streamFormat.mBitsPerChannel == 32,
                      abs(streamFormat.mSampleRate - Double(rate)) < 0.5 else {
                    throw CaptureFailure.message("The aggregate exposes incompatible input stream formats.")
                }
            }
            var quantum: UInt32 = 512
            var property = address(kAudioDevicePropertyBufferFrameSize)
            _ = timedCall("AudioObjectSetPropertyData:fsiz") { AudioObjectSetPropertyData(aggregate, &property, 0, nil, UInt32(MemoryLayout<UInt32>.size), &quantum) }
            quantum = try scalar(aggregate, kAudioDevicePropertyBufferFrameSize, UInt32(0))
            guard let queue = vt_ring_create(rate, micChannels, systemChannels, quantum, epoch, originTicks) else {
                throw CaptureFailure.message("Could not allocate the bounded capture buffer for this input format.")
            }
            ring = queue
            overflow = OverflowCoalescer(maxHeldFrames: UInt64(rate) * 2)
            mic = Accumulator(track: "mic", channels: micChannels, rate: rate)
            system = systemChannels > 0 ? Accumulator(track: "system", channels: systemChannels, rate: rate) : nil
            configurationGate = nil
            refreshOutputRoute(force: true)
            let start = elapsedNow()
            for item in [mic, system].compactMap({ $0 }) {
                emit(["type": "format", "track": item.track, "sample_rate": rate, "channels": item.channels, "epoch": epoch, "start_ns": start])
            }
            try checked(timedCall("AudioDeviceCreateIOProcID") { AudioDeviceCreateIOProcID(aggregate, captureIOProc, UnsafeMutableRawPointer(queue), &ioProc) }, "Create capture callback")
            try checked(timedCall("AudioDeviceStart") { AudioDeviceStart(aggregate, ioProc) }, "Start audio capture")
            started = true
            streamStartedAt = elapsedNow()
            // The baseline is provisional until the started stream settles; a
            // baseline read mid-transition used to cause a second rebuild.
            let opened = OpenedStream(input: input, channels: actualChannels, format: StreamFingerprint(format), streams: openedStreams, quantum: quantum)
            configurationGate = ConfigurationChangeGate(settling: fingerprint(publishedExclusions: true), opened: opened, at: elapsedNow())
            nextSettleCheck = elapsedNow() + ConfigurationChangeGate.settleCheckIntervalNs
            emitDiagnostic("epoch", ["epoch": epoch, "sample_rate": rate, "mic_channels": micChannels, "system_channels": systemChannels, "quantum": quantum,
                                     "input_transport": inputTransport(try? scalar(input, kAudioDevicePropertyTransportType, UInt32(0)))])
            watch(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultInputDevice)
            watch(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice)
            watch(input, kAudioDevicePropertyDeviceIsAlive)
            watch(input, kAudioDevicePropertyNominalSampleRate)
            watch(input, kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyScopeInput)
            if let output = try? scalar(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice, AudioObjectID(0)), output != kAudioObjectUnknown {
                watch(output, kAudioDevicePropertyNominalSampleRate)
                watch(output, kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyScopeOutput)
            }
            watch(aggregate, kAudioDevicePropertyNominalSampleRate)
            watch(aggregate, kAudioDevicePropertyBufferFrameSize)
            watch(aggregate, kAudioDevicePropertyStreamConfiguration, kAudioDevicePropertyScopeInput)
            emit(["type": "status", "message": systemChannels > 0 ? "Microphone and system audio are recording on one aggregate clock." : "Microphone is recording."])
        } catch { stop(); throw error }
    }

    func drain() {
        guard let ring = ring else { return }
        var info = VTFrameInfo()
        while vt_ring_peek(ring, &info) {
            if info.dropped_frames > 0 {
                publish(overflow.drop(epoch: info.epoch, start: info.gap_start_ns, end: info.gap_end_ns, frames: info.dropped_frames))
            }
            var tracks: [[Float]] = []
            for (track, item) in [(UInt32(0), mic), (UInt32(1), system)] {
                guard let item = item else { continue }
                var values = [Float](repeating: 0, count: Int(info.frame_count * item.channels))
                values.withUnsafeMutableBufferPointer { buffer in vt_ring_copy_track(ring, track, buffer.baseAddress!) }
                tracks.append(values)
            }
            let end = info.host_time_ns + UInt64(info.frame_count) * 1_000_000_000 / UInt64(info.sample_rate)
            publish(overflow.packet(CapturedPacket(info: info, tracks: tracks), end: end, frames: UInt64(info.frame_count)))
            vt_ring_pop(ring)
        }
    }

    func publish(_ outputs: [OverflowCoalescer<CapturedPacket>.Output]) {
        for output in outputs {
            switch output {
            case .gap(let gap):
                mic?.flush(); system?.flush()
                emitGap(epoch: gap.epoch, start: gap.start, end: gap.end, reason: "overflow", mic: mic != nil, system: system != nil, overflow: gap)
                lastEnd = max(lastEnd, gap.end)
            case .packet(let packet):
                for (item, values) in zip([mic, system].compactMap({ $0 }), packet.tracks) { item.append(values, packet.info) }
                lastEnd = packet.info.host_time_ns + UInt64(packet.info.frame_count) * 1_000_000_000 / UInt64(packet.info.sample_rate)
            }
        }
    }

    func stop() {
        if started { _ = timedCall("AudioDeviceStop") { AudioDeviceStop(aggregate, ioProc) }; started = false }
        configurationGate = nil
        for (object, var property, block) in listeners { _ = timedCall("AudioObjectRemovePropertyListenerBlock:\(fourCC(property.mSelector))") { AudioObjectRemovePropertyListenerBlock(object, &property, DispatchQueue.global(qos: .userInitiated), block) } }
        listeners.removeAll()
        clearRouteListeners()
        outputObservation = nil
        if let callback = ioProc {
            let removed = timedCall("AudioDeviceDestroyIOProcID") { AudioDeviceDestroyIOProcID(aggregate, callback) }
            if removed != noErr {
                // Never free the C callback context while an IOProc could
                // still be using it. Destroying its aggregate is a second
                // synchronization barrier; if both fail, keep storage alive
                // until process exit and stop instead of risking a use-after-free.
                let destroyed = timedCall("AudioHardwareDestroyAggregateDevice") { AudioHardwareDestroyAggregateDevice(aggregate) }
                if destroyed != noErr {
                    emit(["type": "error", "fatal": true, "message": "The audio callback could not be detached safely. Capture is stopping."])
                    vt_request_stop()
                    return
                }
                aggregate = kAudioObjectUnknown
            }
            ioProc = nil
        }
        drain()
        if let ring = ring {
            var info = VTFrameInfo()
            if vt_ring_final_gap(ring, &info) { publish(overflow.drop(epoch: info.epoch, start: info.gap_start_ns, end: info.gap_end_ns, frames: info.dropped_frames)) }
            vt_ring_destroy(ring); self.ring = nil
        }
        publish(overflow.finish())
        mic?.flush(); system?.flush()
        if aggregate != kAudioObjectUnknown { _ = timedCall("AudioHardwareDestroyAggregateDevice") { AudioHardwareDestroyAggregateDevice(aggregate) }; aggregate = kAudioObjectUnknown }
        if tap != kAudioObjectUnknown { _ = timedCall("AudioHardwareDestroyProcessTap") { AudioHardwareDestroyProcessTap(tap) }; tap = kAudioObjectUnknown }
    }

    func rebuild(reason: String) throws {
        let begin = elapsedNow()
        let cause = trigger ?? RebuildTrigger(source: "unknown", observedAt: begin)
        let fingerprintNs = lastFingerprintNs
        trigger = nil; lastFingerprintNs = 0
        let oldEpoch = epoch
        let hadSystem = system != nil
        diagnosticPhase = "stop"
        stop()
        let stopped = elapsedNow()
        var startedAt = stopped
        var result = "failed"
        defer {
            diagnosticPhase = "idle"
            emitDiagnostic("rebuild", ["reason": reason, "trigger": cause.source, "previous_epoch": oldEpoch, "epoch": epoch,
                                       "trigger_age_ns": begin >= cause.observedAt ? begin - cause.observedAt : 0,
                                       "fingerprint_ns": fingerprintNs, "stop_ns": stopped - begin,
                                       "start_ns": elapsedNow() - startedAt, "result": result])
        }
        guard !vt_stop_requested() else { result = "cancelled"; throw CaptureFailure.message("Capture stopped during device rebuild.") }
        let gapStart = lastEnd
        // Close the old epoch before announcing the replacement format. The
        // first new audio packet supplies its actual hardware start timestamp.
        emitGap(epoch: oldEpoch, start: gapStart, end: stopped, reason: reason, mic: true, system: hadSystem)
        // A further rebuild before new audio arrives continues from this gap
        // instead of re-reporting the old epoch's end.
        lastEnd = max(lastEnd, stopped)
        epoch += 1
        diagnosticPhase = "start"
        startedAt = elapsedNow()
        try start()
        result = "ok"
    }
}

func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else { return nil }
    return CommandLine.arguments[index + 1]
}

func testFingerprints() {
    var raw = AudioStreamBasicDescription()
    raw.mSampleRate = 48_000; raw.mFormatID = kAudioFormatLinearPCM
    raw.mFormatFlags = kAudioFormatFlagIsFloat; raw.mBitsPerChannel = 32
    raw.mChannelsPerFrame = 3; raw.mBytesPerFrame = 12
    let format = StreamFingerprint(raw)
    let baseline = CaptureFingerprint(input: 10, inputAlive: true, inputRate: rateFingerprint(48_000), inputChannels: 1,
        aggregateChannels: 3, aggregateFormat: format, streams: [format], quantum: 512, output: 20, exclusions: [30])
    var gate = ConfigurationChangeGate(baseline: baseline)
    for n in 0..<20 { precondition(gate.observe(baseline, at: UInt64(n) * 10_000_000) == nil) }
    precondition(gate.candidate == nil)

    // Reserved bytes and tiny numerical representation noise are not format changes.
    raw.mReserved = 1
    raw.mSampleRate = 48_000.000_000_1
    precondition(StreamFingerprint(raw) == format)
    raw.mSampleRate = 44_100
    let changedFormat = StreamFingerprint(raw)
    var variants: [CaptureFingerprint] = []
    var changed = baseline; changed.output = 21; variants.append(changed)
    changed = baseline; changed.outputRate = rateFingerprint(16_000); variants.append(changed)
    changed = baseline; changed.outputChannels = 1; variants.append(changed)
    changed = baseline; changed.input = 11; variants.append(changed)
    changed = baseline; changed.inputRate = rateFingerprint(44_100); variants.append(changed)
    changed = baseline; changed.inputChannels = 2; variants.append(changed)
    changed = baseline; changed.aggregateChannels = 4; variants.append(changed)
    changed = baseline; changed.aggregateFormat = changedFormat; variants.append(changed)
    changed = baseline; changed.streams = [changedFormat]; variants.append(changed)
    changed = baseline; changed.quantum = 1_024; variants.append(changed)
    changed = baseline; changed.exclusions = [30, 31]; variants.append(changed)
    for variant in variants {
        var check = ConfigurationChangeGate(baseline: baseline)
        precondition(check.observe(variant, at: 0) == nil)
        precondition(check.observe(variant, at: ConfigurationChangeGate.stabilityWindowNs - 1) == nil)
        let reason = check.observe(variant, at: ConfigurationChangeGate.stabilityWindowNs)
        precondition(reason == (variant.exclusions == baseline.exclusions ? "device_change" : "process_exclusion_change"))
    }

    // A transient notification/query result that returns to baseline never rebuilds.
    changed = baseline; changed.quantum = nil
    gate = ConfigurationChangeGate(baseline: baseline)
    precondition(gate.observe(changed, at: 0) == nil)
    precondition(gate.observe(baseline, at: 20_000_000) == nil)
    precondition(gate.candidate == nil)
    // A new value within a burst restarts the stability window.
    changed = baseline; changed.output = 21
    precondition(gate.observe(changed, at: 0) == nil)
    changed.output = 22
    precondition(gate.observe(changed, at: 50_000_000) == nil)
    precondition(gate.observe(changed, at: 100_000_000) == nil)
    precondition(gate.observe(changed, at: 125_000_000) == "device_change")
    // Confirmed device death is immediate; a missing/uncertain query is not proof.
    gate = ConfigurationChangeGate(baseline: baseline)
    changed = baseline; changed.inputAlive = false
    precondition(gate.observe(changed, at: 0) == "device_change")
    changed = baseline; changed.input = nil; changed.inputAlive = nil
    gate = ConfigurationChangeGate(baseline: baseline)
    precondition(gate.observe(changed, at: 0) == nil)
    precondition(gate.observe(changed, at: 75_000_000) == "device_change")
    testSettlingBaseline(baseline: baseline, format: format, changedFormat: changedFormat)
    emit(["type": "status", "message": "Capture fingerprint tests passed: unchanged notifications, route/format/exclusion changes, transient reads, debounce bursts, immediate device death, and one rebuild per device change while a new stream settles."])
}

func testSettlingBaseline(baseline: CaptureFingerprint, format: StreamFingerprint, changedFormat: StreamFingerprint) {
    let opened = OpenedStream(input: 10, channels: 3, format: format, streams: [format], quantum: 512)
    let window = ConfigurationChangeGate.stabilityWindowNs
    let minimum = ConfigurationChangeGate.settleMinimumNs
    // Reproduce the field sequence: the baseline is read mid-transition, then
    // our own start renegotiates the output (Bluetooth call profile) and a
    // query briefly fails. Previously the settled values differed from the
    // baseline and caused a second rebuild; now exactly one rebuild happens.
    var settled = baseline; settled.output = 21; settled.outputRate = rateFingerprint(16_000); settled.outputChannels = 1
    settled.inputRate = rateFingerprint(16_000); settled.quantum = 256
    var transient = settled; transient.quantum = nil; transient.aggregateFormat = nil
    var gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    var legacy = ConfigurationChangeGate(baseline: baseline)
    var rebuilds = 0, legacyRebuilt = false
    for (n, value) in [baseline, transient, settled, settled, settled, settled, settled, settled].enumerated() {
        let now = UInt64(n) * 250_000_000
        if gate.observe(value, at: now) != nil { rebuilds += 1 }
        if !legacyRebuilt && legacy.observe(value, at: now) != nil { legacyRebuilt = true }
    }
    precondition(legacyRebuilt, "the legacy fixed baseline reproduces the spurious second rebuild")
    precondition(rebuilds == 0 && !gate.settling && gate.settledStable && gate.absorbed == 2)
    // Failed reads did not erase settled values, so later reads do not rebuild.
    precondition(gate.baseline.quantum == 256 && gate.baseline.aggregateFormat == format)
    for n in 0..<20 { precondition(gate.observe(n % 2 == 0 ? settled : transient, at: 2_000_000_000 + UInt64(n) * window) == nil) }
    // Once armed, a real later change still rebuilds through the normal window.
    var later = settled; later.output = 22
    precondition(gate.observe(later, at: 10_000_000_000) == nil)
    precondition(gate.observe(later, at: 10_000_000_000 + window) == "device_change")

    // Not settled before the minimum time, even when values are stable.
    gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    precondition(gate.observe(baseline, at: minimum - 1) == nil && gate.settling)
    precondition(gate.observe(baseline, at: minimum) == nil && !gate.settling && gate.absorbed == 0)
    // A value still changing at the minimum keeps the gate settling until stable.
    gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    var moving = baseline; moving.outputRate = rateFingerprint(44_100)
    precondition(gate.observe(moving, at: minimum) == nil && gate.settling)
    precondition(gate.observe(moving, at: minimum + window) == nil && !gate.settling && gate.settledStable)
    // A stream that never settles is armed at the maximum, marked unstable.
    gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    var step: UInt64 = 0
    while gate.settling {
        step += 1
        var flapping = baseline; flapping.outputChannels = UInt32(step % 2 + 1)
        precondition(gate.observe(flapping, at: step * 50_000_000) == nil)
        precondition(step * 50_000_000 <= ConfigurationChangeGate.settleMaximumNs)
    }
    precondition(!gate.settledStable)

    // Changes to what the running ring was opened for are never absorbed.
    var invalidating: [CaptureFingerprint] = []
    var changed = baseline; changed.input = 11; invalidating.append(changed)
    changed = baseline; changed.aggregateChannels = 4; invalidating.append(changed)
    changed = baseline; changed.aggregateFormat = changedFormat; invalidating.append(changed)
    changed = baseline; changed.streams = [changedFormat]; invalidating.append(changed)
    changed = baseline; changed.quantum = 1_024; invalidating.append(changed)
    for value in invalidating {
        gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
        precondition(gate.observe(value, at: 100) == nil)
        precondition(gate.observe(value, at: 100 + window) == "device_change")
    }
    // A smaller quantum fits the ring and is absorbed.
    gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    changed = baseline; changed.quantum = 128
    precondition(gate.observe(changed, at: 100) == nil && gate.observe(changed, at: 100 + window) == nil)
    // Confirmed device death is immediate even while settling.
    gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    changed = baseline; changed.inputAlive = false
    precondition(gate.observe(changed, at: 0) == "device_change")
    // An exclusion change seen while settling is not absorbed: once armed it
    // rebuilds with its own reason because published exclusions stay in the baseline.
    gate = ConfigurationChangeGate(settling: baseline, opened: opened, at: 0)
    changed = baseline; changed.exclusions = [30, 31]
    precondition(gate.observe(changed, at: minimum) == nil && !gate.settling && gate.baseline.exclusions == [30])
    precondition(gate.observe(changed, at: minimum + 1) == nil)
    precondition(gate.observe(changed, at: minimum + 1 + window) == "process_exclusion_change")
}

func testOverflowCoalescing() {
    typealias Coalescer = OverflowCoalescer<Int>
    let rate: UInt64 = 8_000
    func ns(_ frames: UInt64) -> UInt64 { frames * 1_000_000_000 / rate }
    // Synthetic contiguous timeline in frames: packets are 80 frames (10 ms).
    struct Script { var coalescer = Coalescer(maxHeldFrames: 16_000); var out: [Coalescer.Output] = []; var next = 0 }
    func packet(_ s: inout Script, at frame: UInt64) { s.out += s.coalescer.packet(s.next, end: ns(frame + 80), frames: 80); s.next += 1 }
    func drop(_ s: inout Script, at frame: UInt64, frames: UInt64) { s.out += s.coalescer.drop(epoch: 2, start: ns(frame), end: ns(frame + frames), frames: frames) }
    func gaps(_ s: Script) -> [OverflowGap] { s.out.compactMap { if case .gap(let g) = $0 { return g } else { return nil } } }
    func packets(_ s: Script) -> [Int] { s.out.compactMap { if case .packet(let p) = $0 { return p } else { return nil } } }

    // No overflow: packets pass through immediately.
    var s = Script()
    packet(&s, at: 0); packet(&s, at: 80)
    precondition(packets(s) == [0, 1] && gaps(s).isEmpty)

    // A ten-run burst (alternating one delivered quantum and a drop, as during
    // ring saturation) becomes one gap: first drop start to last drop end.
    s = Script()
    packet(&s, at: 0)
    var frame: UInt64 = 80
    var dropped: UInt64 = 0
    for _ in 0..<10 {
        drop(&s, at: frame, frames: 160); frame += 160; dropped += 160
        packet(&s, at: frame); frame += 80
    }
    precondition(gaps(s).isEmpty && packets(s) == [0], "the burst is held until it closes")
    // One second of clean audio closes the burst and releases held packets in order.
    let lastDropEnd = frame - 80
    while ns(frame) - ns(lastDropEnd) < Coalescer.windowNs { packet(&s, at: frame); frame += 80 }
    let burst = gaps(s)
    precondition(burst.count == 1)
    let gap = burst[0]
    precondition(gap.start == ns(80) && gap.end == ns(lastDropEnd) && gap.runs == 10 && gap.epoch == 2)
    // Exact accounting: 10 dropped runs plus the 9 delivered quanta between them.
    precondition(gap.droppedFrames == dropped && gap.discardedFrames == 9 * 80)
    precondition(gap.end - gap.start == ns(gap.droppedFrames + gap.discardedFrames))
    let released = packets(s)
    precondition(released.first == 0 && released[1] == 10 && released == released.sorted())
    precondition(!released.contains(1) && !released.contains(9), "audio inside the gap is discarded, never published")
    if case .gap = s.out[1] {} else { preconditionFailure("the gap precedes the released audio") }

    // Runs separated by one second or more of clean audio stay separate gaps.
    s = Script()
    drop(&s, at: 0, frames: 80)
    frame = 80
    while ns(frame) - ns(80) < Coalescer.windowNs { packet(&s, at: frame); frame += 80 }
    drop(&s, at: frame, frames: 80); frame += 80
    packet(&s, at: frame)
    s.out += s.coalescer.finish()
    precondition(gaps(s).count == 2 && gaps(s).allSatisfy { $0.runs == 1 && $0.discardedFrames == 0 })
    precondition(packets(s).count == s.next, "no audio between separate gaps is lost")

    // finish() (stop/rebuild) publishes a pending gap, including a final unqueued drop.
    s = Script()
    drop(&s, at: 0, frames: 80); packet(&s, at: 80); drop(&s, at: 160, frames: 40)
    s.out += s.coalescer.finish()
    precondition(gaps(s) == [OverflowGap(epoch: 2, start: 0, end: ns(200), droppedFrames: 120, discardedFrames: 80, runs: 2)])
    precondition(packets(s).isEmpty && s.coalescer.pending == nil && s.coalescer.finish().isEmpty)

    // Held audio is bounded even when host timestamps do not advance.
    var bounded = Coalescer(maxHeldFrames: 160)
    _ = bounded.drop(epoch: 0, start: 0, end: 10, frames: 1)
    precondition(bounded.packet(0, end: 10, frames: 80).isEmpty)
    precondition(bounded.packet(1, end: 10, frames: 80).count == 3)
    emit(["type": "status", "message": "Overflow coalescing tests passed: pass-through, one gap per burst with exact dropped/discarded accounting, separate distant runs, final drops and bounded holding."])
}

func testDiagnostics() {
    precondition(fourCC(kAudioDevicePropertyNominalSampleRate) == "nsrt")
    precondition(fourCC(kAudioHardwarePropertyProcessObjectList) == "prs#")
    precondition(fourCC(0x0102_0304) == "01020304")
    let expected: [UInt32: String] = [kAudioDeviceTransportTypeBuiltIn: "built_in", kAudioDeviceTransportTypeBluetooth: "bluetooth",
        kAudioDeviceTransportTypeBluetoothLE: "bluetooth", kAudioDeviceTransportTypeUSB: "usb",
        kAudioDeviceTransportTypeAggregate: "aggregate", kAudioDeviceTransportTypeVirtual: "virtual",
        kAudioDeviceTransportTypeHDMI: "other", kAudioDeviceTransportTypeDisplayPort: "other",
        kAudioDeviceTransportTypeThunderbolt: "other", kAudioDeviceTransportTypePCI: "other", 0: "unknown"]
    for (value, name) in expected { precondition(inputTransport(value) == name) }
    precondition(inputTransport(nil) == "unknown")
    let event = diagnosticEvent("heartbeat", ["callbacks": 3])
    precondition(event["type"] as? String == "status" && event["message"] is String)
    precondition((event["diagnostic"] as? [String: Any])?["kind"] as? String == "heartbeat")
    // Read-only probes never publish diagnostics (their stdout is one document).
    precondition(!diagnosticsEnabled)
    var trigger = RebuildTrigger.from(sources: UInt32(VT_REBUILD_POLL | VT_REBUILD_NOTIFICATION), firstTicks: 0, now: 5)
    precondition(trigger.source == "notification" && trigger.observedAt == 5)
    trigger = RebuildTrigger.from(sources: UInt32(VT_REBUILD_WAKE | VT_REBUILD_POLL), firstTicks: originTicks, now: 5)
    precondition(trigger.source == "wake" && trigger.observedAt == 0)
    precondition(RebuildTrigger.from(sources: 0, firstTicks: 0, now: 1).source == "unknown")
    // Rebuild requests keep their first observation time and every source until taken.
    _ = vt_take_rebuild()
    vt_request_rebuild_from(UInt32(VT_REBUILD_POLL)); vt_request_rebuild()
    var sources: UInt32 = 0, first: UInt64 = 0
    precondition(vt_take_rebuild_info(&sources, &first) && sources == UInt32(VT_REBUILD_POLL | VT_REBUILD_NOTIFICATION) && first > 0)
    precondition(!vt_take_rebuild_info(&sources, &first))
    let callbacks = vt_callback_count(); vt_count_callback()
    precondition(vt_callback_count() == callbacks + 1)
    emit(["type": "status", "message": "Diagnostic tests passed: selector codes, approved input transport classes, status-wrapped events, probe silence, rebuild trigger sources and the callback counter."])
}

func testOutputRoutes() {
    var verified = OutputRoute()
    verified.transport = "built_in"; verified.default_output_known = true
    verified.data_source_known = true; verified.speaker_terminal = true
    verified.headphone_jack_connected = false
    verified.active_output_processes = 1; verified.process_devices_known = true
    verified.all_processes_same_output = true
    verified.global_tap = true; verified.reference_available = true
    verified.classify()
    precondition(verified.kind == "built_in_speakers" && verified.reason == "eligible")
    var changed = verified; changed.headphone_jack_connected = true; changed.classify()
    precondition(changed.kind == "headphones" && changed.reason == "headphones")
    changed = verified; changed.headphone_jack_connected = nil; changed.classify()
    precondition(changed.kind == "unknown" && changed.reason == "output_evidence_unavailable")
    changed = verified; changed.speaker_terminal = false; changed.classify()
    precondition(changed.reason == "output_evidence_unavailable")
    changed = verified; changed.data_source_known = false; changed.classify()
    precondition(changed.reason == "output_evidence_unavailable")
    changed = verified; changed.default_output_known = false; changed.classify()
    precondition(changed.reason == "output_evidence_unavailable")
    for transport in ["usb", "bluetooth", "hdmi", "display_port", "aggregate", "virtual", "other"] {
        changed = verified; changed.transport = transport; changed.classify()
        precondition(changed.kind == "external" && changed.reason == "external_output")
    }
    changed = verified; changed.transport = "unknown"; changed.classify()
    precondition(changed.reason == "output_evidence_unavailable")
    changed = verified; changed.process_devices_known = false; changed.classify()
    precondition(changed.reason == "process_evidence_unavailable")
    changed = verified; changed.all_processes_same_output = false; changed.classify()
    precondition(changed.reason == "mixed_process_outputs")
    changed = verified; changed.active_output_processes = 0; changed.classify()
    precondition(changed.reason == "no_active_output")
    changed = verified; changed.reference_available = false; changed.classify()
    precondition(changed.reason == "no_reference")
    changed = verified; changed.global_tap = false; changed.classify()
    precondition(changed.reason == "no_reference")
    // An application's nondefault route changes the local observation even if
    // the default device and transport remain unchanged. Identifiers stay local.
    let a = ProcessOutputRoute(process: 30, running: 1, devices: [20])
    let b = ProcessOutputRoute(process: 30, running: 1, devices: [21])
    let first = OutputRouteObservation(output: 20, sources: [1], terminals: [kAudioStreamTerminalTypeSpeaker], processes: [a], route: verified)
    let second = OutputRouteObservation(output: 20, sources: [1], terminals: [kAudioStreamTerminalTypeSpeaker], processes: [b], route: verified)
    precondition(first != second)
    let idle = ProcessOutputRoute(process: 31, running: 0, devices: nil)
    changed = verified
    applyProcessEvidence([a, idle], output: 20, route: &changed); changed.classify()
    precondition(changed.reason == "eligible" && changed.active_output_processes == 1)
    for list: [ProcessOutputRoute]? in [nil, [ProcessOutputRoute(process: 30, running: nil, devices: nil)], [ProcessOutputRoute(process: 30, running: 1, devices: [])]] {
        changed = verified; applyProcessEvidence(list, output: 20, route: &changed); changed.classify()
        precondition(changed.reason == "process_evidence_unavailable")
    }
    for list in [[a, b], [ProcessOutputRoute(process: 30, running: 1, devices: [20, 21])], [b]] {
        changed = verified; applyProcessEvidence(list, output: 20, route: &changed); changed.classify()
        precondition(changed.reason == "mixed_process_outputs")
    }
    changed = verified; applyProcessEvidence([idle], output: 20, route: &changed); changed.classify()
    precondition(changed.reason == "no_active_output")
    changed = verified; applyProcessEvidence([a], output: nil, route: &changed); changed.classify()
    precondition(changed.reason != "eligible")
    precondition(!verified.json.keys.contains("process") && !verified.json.keys.contains("output"))
    emit(["type": "status", "message": "Output-route policy tests passed: speaker proof, connected/unknown jack, unavailable data/terminal evidence, external transports, missing/mixed process routes and reference reset."])
}

func runCapture() throws {
    guard #available(macOS 14.4, *) else { throw CaptureFailure.message("System capture requires macOS 14.4 or newer.") }
    let mode = argument("--mode") ?? "remote"
    guard mode == "remote" || mode == "in_person" else { throw CaptureFailure.message("Mode must be remote or in_person.") }
    let parentPID = Int32(argument("--parent-pid") ?? "") ?? getppid()
    signal(SIGPIPE, SIG_IGN)
    diagnosticsEnabled = true
    signal(SIGTERM) { _ in vt_request_stop() }
    signal(SIGINT) { _ in vt_request_stop() }
    DispatchQueue.global(qos: .utility).async {
        while let line = readLine() { if line.trimmingCharacters(in: .whitespacesAndNewlines) == "stop" { break } }
        vt_request_stop()
    }
    // Same raw-uptime epoch as mach_absolute_time/mHostTime, in nanoseconds.
    // Emitted before permission prompts: pipe delivery time is never the origin.
    emit(["type": "source_clock", "clock": "uptime_raw", "origin_ns": vt_elapsed_ns(originTicks, 0), "observed_ns": clock_gettime_nsec_np(CLOCK_UPTIME_RAW)])
    try microphonePermission()
    let engine = CaptureEngine(parentPID: parentPID, micUID: argument("--mic-device"), remote: mode == "remote")
    var assertion: IOPMAssertionID = 0
    let asserted = IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleSystemSleep as CFString, IOPMAssertionLevel(kIOPMAssertionLevelOn), "VanTranscribe recording" as CFString, &assertion) == kIOReturnSuccess
    defer { engine.stop(); if asserted { IOPMAssertionRelease(assertion) } }
    try engine.start()
    let center = NSWorkspace.shared.notificationCenter
    let sleepObserver = center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { _ in engine.suspended = true; engine.stop(); emit(["type": "status", "message": "Capture paused while the computer sleeps."]) }
    let wakeObserver = center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { _ in engine.suspended = false; vt_request_rebuild_from(UInt32(VT_REBUILD_WAKE)) }
    defer { center.removeObserver(sleepObserver); center.removeObserver(wakeObserver) }
    var nextConfigurationCheck = elapsedNow() + 2_000_000_000
    var nextRouteCheck: UInt64 = 0
    var nextHeartbeat = elapsedNow() + 1_000_000_000
    var controlTicks: UInt64 = 0
    while !vt_stop_requested() {
        controlTicks &+= 1
        if !engine.suspended && elapsedNow() >= nextRouteCheck {
            nextRouteCheck = elapsedNow() + 250_000_000
            diagnosticPhase = "route"
            engine.refreshOutputRoute()
            diagnosticPhase = "idle"
        }
        if !engine.suspended {
            var sources: UInt32 = 0
            var firstTicks: UInt64 = 0
            let requested = vt_take_rebuild_info(&sources, &firstTicks)
            if requested {
                engine.noteTrigger(RebuildTrigger.from(sources: sources, firstTicks: firstTicks, now: elapsedNow()))
                diagnosticPhase = "route"
                engine.refreshOutputRoute()
                diagnosticPhase = "idle"
            }
            if requested && !engine.started {
                try engine.rebuild(reason: "device_change")
            } else if requested || engine.hasPendingConfigurationChange || engine.settleCheckDue(elapsedNow()) {
                engine.noteTrigger(RebuildTrigger(source: "settle_check", observedAt: elapsedNow()))
                if let reason = engine.configurationChangeReason() { try engine.rebuild(reason: reason) }
                else { engine.forgetIdleTrigger() }
            }
        }
        engine.drain()
        if !engine.suspended && elapsedNow() >= nextConfigurationCheck {
            nextConfigurationCheck = elapsedNow() + 2_000_000_000
            // A periodic value check also catches missed native notifications.
            // It is safe in both modes because unchanged values do not rebuild.
            vt_request_rebuild_from(UInt32(VT_REBUILD_POLL))
        }
        let now = elapsedNow()
        if now >= nextHeartbeat {
            // Emit-only liveness evidence: callbacks advance on the IOProc,
            // control_ticks on this loop, max_emit_ns grows when stdout blocks.
            nextHeartbeat = now + 1_000_000_000
            emitDiagnostic("heartbeat", ["epoch": engine.epoch, "callbacks": vt_callback_count(), "control_ticks": controlTicks,
                                         "queued_packets": engine.ring.map { vt_ring_queued($0) } ?? 0, "max_emit_ns": maxEmitNs, "at_ns": now])
            maxEmitNs = 0
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
    }
    engine.stop()
    emit(["type": "status", "message": "Capture stopped."])
}

let supportedCommands = ["capture", "list_devices", "meeting_apps", "output_route", "test_fingerprints", "test_output_routes", "test_arguments", "test_source_clock", "test_meeting_apps", "test_overflow_coalescing", "test_diagnostics", "version", "help"]
// `capture_diagnostics`: capture publishes status events with a typed `diagnostic`.
let captureCapabilities = ["source_clock_uptime_raw", "capture_diagnostics"]
enum HelperCommand: Equatable {
    case capture, listDevices, meetingApps, outputRoute, testFingerprints, testOutputRoutes, testArguments, testSourceClock, testMeetingApps, testOverflowCoalescing, testDiagnostics, version, help
}

// Parse every argument before querying permissions or creating any audio object.
// Capture must be explicit: an unknown future probe flag can never start it.
func validatedCommand(_ args: [String]) throws -> HelperCommand {
    let probes: [String: HelperCommand] = [
        "--list-devices": .listDevices, "--meeting-apps": .meetingApps, "--output-route": .outputRoute,
        "--test-fingerprints": .testFingerprints, "--test-output-routes": .testOutputRoutes,
        "--test-meeting-apps": .testMeetingApps, "--test-arguments": .testArguments, "--test-source-clock": .testSourceClock,
        "--test-overflow-coalescing": .testOverflowCoalescing, "--test-diagnostics": .testDiagnostics, "--version": .version, "--help": .help
    ]
    if args.count == 1, let probe = probes[args[0]] { return probe }
    var seen = Set<String>()
    var i = 0
    while i < args.count {
        let key = args[i]
        guard ["--mode", "--parent-pid", "--mic-device"].contains(key), seen.insert(key).inserted else {
            throw CaptureFailure.message("Unknown, repeated or incompatible command option. Use --help or select --mode explicitly.")
        }
        guard i + 1 < args.count, !args[i + 1].hasPrefix("--"), !args[i + 1].trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw CaptureFailure.message("A capture option is missing its value. No capture was started.")
        }
        let value = args[i + 1]
        if key == "--mode" && value != "remote" && value != "in_person" {
            throw CaptureFailure.message("Mode must be remote or in_person. No capture was started.")
        }
        if key == "--parent-pid" && (Int32(value).map { $0 > 0 } != true) {
            throw CaptureFailure.message("Parent PID must be a positive process ID. No capture was started.")
        }
        i += 2
    }
    guard seen.contains("--mode") else {
        throw CaptureFailure.message("Capture requires explicit --mode remote or --mode in_person. No capture was started.")
    }
    return .capture
}

func testArguments() {
    let invalid: [[String]] = [[], ["--unknown-probe"], ["--mode"], ["--mode", "--list-devices"],
        ["--mode", "invalid"], ["--mic-device"], ["--parent-pid", ""], ["--parent-pid", "oops", "--mode", "remote"],
        ["--mode", "remote", "--mic-device"], ["--mode", "remote", "--mic-device", " "],
        ["--mode", "remote", "--parent-pid", "0"], ["--mode", "remote", "--parent-pid", "999999999999999"],
        ["--mode", "remote", "--mode", "in_person"], ["--mode", "remote", "extra"],
        ["--meeting-apps", "--mode", "remote"], ["--meeting-apps", "--parent-pid", "1"], ["--meeting-apps", "--meeting-apps"], ["--test-meeting-apps", "--mode", "remote"], ["--output-route", "--mode", "remote"], ["--list-devices", "--unknown"], ["--version", "--help"]]
    for args in invalid {
        do { _ = try validatedCommand(args); preconditionFailure("Invalid CLI arguments were accepted") }
        catch { }
    }
    precondition(try! validatedCommand(["--mode", "remote"]) == .capture)
    precondition(try! validatedCommand(["--mic-device", "synthetic-uid", "--mode", "in_person", "--parent-pid", "123"]) == .capture)
    precondition(try! validatedCommand(["--list-devices"]) == .listDevices)
    precondition(try! validatedCommand(["--output-route"]) == .outputRoute)
    precondition(try! validatedCommand(["--meeting-apps"]) == .meetingApps)
    precondition(try! validatedCommand(["--test-meeting-apps"]) == .testMeetingApps)
    precondition(try! validatedCommand(["--version"]) == .version)
    precondition(try! validatedCommand(["--test-overflow-coalescing"]) == .testOverflowCoalescing)
    precondition(try! validatedCommand(["--test-diagnostics"]) == .testDiagnostics)
    for args in [["--test-diagnostics", "--mode", "remote"], ["--test-overflow-coalescing", "--mode", "remote"]] {
        do { _ = try validatedCommand(args); preconditionFailure("Mixed self-test and capture arguments were accepted") } catch { }
    }
    emit(["type": "status", "message": "CLI validation tests passed: explicit capture mode, unknown/repeated/mixed options, missing values and invalid parent IDs are rejected before capture."])
}

func testSourceClock() {
    for _ in 0..<1000 {
        let before = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        let absolute = vt_elapsed_ns(mach_absolute_time(), 0)
        let after = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        precondition(before <= absolute && absolute <= after)
    }
    emit(["type": "status", "message": "Source-clock tests passed: hardware Mach ticks and raw-uptime nanoseconds share one monotonic epoch."])
}

do {
    switch try validatedCommand(Array(CommandLine.arguments.dropFirst())) {
    case .testMeetingApps: testMeetingApps()
    case .meetingApps: emit(observeMeetingApps().json)
    case .testOutputRoutes: testOutputRoutes()
    case .outputRoute: emit(observeOutputRoute(exclusions: [], referenceAvailable: false).route.json)
    case .testFingerprints: testFingerprints()
    case .testArguments: testArguments()
    case .testSourceClock: testSourceClock()
    case .testOverflowCoalescing: testOverflowCoalescing()
    case .testDiagnostics: testDiagnostics()
    case .listDevices: try listDevices()
    case .version: emit(["helper_protocol": 4, "supported_commands": supportedCommands, "capture_capabilities": captureCapabilities])
    case .help: emit(["helper_protocol": 4, "supported_commands": supportedCommands, "capture_capabilities": captureCapabilities, "usage": "Capture: --mode remote|in_person [--parent-pid positive-pid] [--mic-device uid]. Read-only commands must be used alone."])
    case .capture: try runCapture()
    }
} catch {
    emit(["type": "error", "fatal": true, "message": String(describing: error)])
    exit(1)
}
