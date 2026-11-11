#ifndef VT_AUDIO_RING_H
#define VT_AUDIO_RING_H
#include <CoreAudio/CoreAudio.h>
#include <stdbool.h>
#include <stdint.h>
typedef struct VTAudioRing VTAudioRing;
typedef struct {
    uint32_t epoch, sample_rate, mic_channels, system_channels, frame_count;
    uint64_t first_frame, host_time_ns;
    uint64_t dropped_frames, gap_start_ns, gap_end_ns;
} VTFrameInfo;
VTAudioRing *vt_ring_create(uint32_t sample_rate, uint32_t mic_channels,
                           uint32_t system_channels, uint32_t max_frames,
                           uint32_t epoch, uint64_t origin_ticks);
void vt_ring_destroy(VTAudioRing *ring);
// Called only by the serialized Core Audio IOProc. No allocation, locks or IO.
void vt_ring_write(VTAudioRing *ring, const AudioBufferList *input, uint64_t host_ticks);
bool vt_ring_peek(VTAudioRing *ring, VTFrameInfo *info);
void vt_ring_copy_track(VTAudioRing *ring, uint32_t track, float *destination);
void vt_ring_pop(VTAudioRing *ring);
// Call only after AudioDeviceStop: returns a final unqueued overflow range.
bool vt_ring_final_gap(VTAudioRing *ring, VTFrameInfo *info);
void vt_request_stop(void);
bool vt_stop_requested(void);
// Rebuild requests carry diagnostic sources (bit flags) and the first request time.
enum { VT_REBUILD_NOTIFICATION = 1, VT_REBUILD_POLL = 2, VT_REBUILD_WAKE = 4 };
void vt_request_rebuild(void);
void vt_request_rebuild_from(uint32_t source);
bool vt_take_rebuild(void);
// Either pointer may be NULL. Zero outputs mean the timing was not captured.
bool vt_take_rebuild_info(uint32_t *sources, uint64_t *first_ticks);
// Lock-free, allocation-free: safe from the IOProc. Monotonic for the process lifetime.
void vt_count_callback(void);
uint64_t vt_callback_count(void);
// Packets written but not yet popped (control-thread backlog).
uint64_t vt_ring_queued(VTAudioRing *ring);
uint64_t vt_elapsed_ns(uint64_t ticks, uint64_t origin_ticks);
int32_t vt_parent_pid(int32_t pid);
// Optional OS responsibility attribution; -1 means unavailable/unresolved.
int32_t vt_responsible_pid(int32_t pid);
uint32_t vt_meeting_app_count(void);
const char *vt_meeting_app_id(uint32_t index);
const char *vt_meeting_app_name(uint32_t index);
const char *vt_meeting_app_kind(uint32_t index);
#endif
