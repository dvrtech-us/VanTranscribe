#include "AudioRing.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <mach/mach_time.h>
#include <libproc.h>
#include <dlfcn.h>
_Static_assert(ATOMIC_LLONG_LOCK_FREE == 2 && ATOMIC_LONG_LOCK_FREE == 2 && ATOMIC_BOOL_LOCK_FREE == 2,
               "The realtime ring requires lock-free atomics");

typedef struct { VTFrameInfo info; float *samples; } VTSlot;
struct VTAudioRing {
    uint32_t rate, mic_channels, system_channels, max_frames, epoch, capacity;
    uint64_t origin_ticks, first_frame, dropped_frames, gap_start_ns, gap_end_ns;
    mach_timebase_info_data_t timebase;
    _Atomic uint64_t write_index, read_index;
    VTSlot *slots;
    float *storage;
};
static _Atomic bool stopped = false, rebuild = false;
static _Atomic uint32_t rebuild_sources = 0;
static _Atomic uint64_t rebuild_first_ticks = 0, callbacks = 0;
void vt_request_stop(void) { atomic_store_explicit(&stopped, true, memory_order_release); }
bool vt_stop_requested(void) { return atomic_load_explicit(&stopped, memory_order_acquire); }
void vt_request_rebuild_from(uint32_t source) {
    // Diagnostics only: remember the first request time and every source until taken.
    uint64_t expected = 0, now = mach_absolute_time();
    atomic_fetch_or_explicit(&rebuild_sources, source, memory_order_relaxed);
    atomic_compare_exchange_strong_explicit(&rebuild_first_ticks, &expected, now ? now : 1,
                                            memory_order_relaxed, memory_order_relaxed);
    atomic_store_explicit(&rebuild, true, memory_order_release);
}
void vt_request_rebuild(void) { vt_request_rebuild_from(VT_REBUILD_NOTIFICATION); }
bool vt_take_rebuild(void) { return vt_take_rebuild_info(NULL, NULL); }
bool vt_take_rebuild_info(uint32_t *sources, uint64_t *first_ticks) {
    if (!atomic_exchange_explicit(&rebuild, false, memory_order_acq_rel)) return false;
    // A request racing this take may leave its source for the next take; the
    // caller treats zero sources/ticks as unknown timing, never as no request.
    uint32_t s = atomic_exchange_explicit(&rebuild_sources, 0, memory_order_relaxed);
    uint64_t t = atomic_exchange_explicit(&rebuild_first_ticks, 0, memory_order_relaxed);
    if (sources) *sources = s;
    if (first_ticks) *first_ticks = t;
    return true;
}
void vt_count_callback(void) { atomic_fetch_add_explicit(&callbacks, 1, memory_order_relaxed); }
uint64_t vt_callback_count(void) { return atomic_load_explicit(&callbacks, memory_order_relaxed); }
uint64_t vt_elapsed_ns(uint64_t ticks, uint64_t origin) {
    mach_timebase_info_data_t timebase; mach_timebase_info(&timebase);
    if (ticks < origin) return 0;
    return (uint64_t)(((__uint128_t)(ticks-origin)*timebase.numer)/timebase.denom);
}
int32_t vt_parent_pid(int32_t pid) {
    struct proc_bsdinfo info;
    int size=proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info));
    return size==(int)sizeof(info) ? (int32_t)info.pbi_ppid : -1;
}
int32_t vt_responsible_pid(int32_t pid) {
    // Not guaranteed by the public SDK: never link this symbol unconditionally.
    typedef pid_t (*ResponsiblePid)(pid_t);
    ResponsiblePid resolve = (ResponsiblePid)dlsym(RTLD_DEFAULT, "responsibility_get_pid_responsible_for_pid");
    return resolve ? (int32_t)resolve((pid_t)pid) : -1;
}
typedef struct { const char *id, *name, *kind; } VTMeetingApp;
static const VTMeetingApp meeting_apps[] = {
#define VT_MEETING_APP(id, name, kind) {id, name, kind},
#include "../../crates/vt-capture/resources/meeting-apps.def"
#undef VT_MEETING_APP
};
uint32_t vt_meeting_app_count(void) { return sizeof(meeting_apps)/sizeof(meeting_apps[0]); }
const char *vt_meeting_app_id(uint32_t i) { return i<vt_meeting_app_count() ? meeting_apps[i].id : NULL; }
const char *vt_meeting_app_name(uint32_t i) { return i<vt_meeting_app_count() ? meeting_apps[i].name : NULL; }
const char *vt_meeting_app_kind(uint32_t i) { return i<vt_meeting_app_count() ? meeting_apps[i].kind : NULL; }
VTAudioRing *vt_ring_create(uint32_t rate, uint32_t mic, uint32_t system, uint32_t max_frames, uint32_t epoch, uint64_t origin) {
    if (rate<8000 || rate>192000 || mic>8 || system>2 || mic+system==0 || max_frames==0 || max_frames>8192) return NULL;
    VTAudioRing *q=calloc(1,sizeof(*q)); if (!q) return NULL;
    q->rate=rate; q->mic_channels=mic; q->system_channels=system; q->max_frames=max_frames;
    q->epoch=epoch; q->origin_ticks=origin;
    q->capacity=(rate*10+max_frames-1)/max_frames+2; // >=10 s at the configured IO quantum.
    q->slots=calloc(q->capacity,sizeof(VTSlot));
    size_t floats=(size_t)q->capacity*max_frames*(mic+system);
    q->storage=calloc(floats,sizeof(float));
    if (!q->slots || !q->storage) { vt_ring_destroy(q); return NULL; }
    for(uint32_t i=0;i<q->capacity;i++) q->slots[i].samples=q->storage+(size_t)i*max_frames*(mic+system);
    mach_timebase_info(&q->timebase);
    // Touch allocated pages before the real-time callback starts.
    for(size_t i=0;i<floats;i+=1024) q->storage[i]=0;
    return q;
}
void vt_ring_destroy(VTAudioRing *q) { if(q){free(q->storage);free(q->slots);free(q);} }
static uint64_t elapsed(const VTAudioRing *q,uint64_t ticks) {
    if(ticks<q->origin_ticks)return 0;
    return (uint64_t)(((__uint128_t)(ticks-q->origin_ticks)*q->timebase.numer)/q->timebase.denom);
}
void vt_ring_write(VTAudioRing *q,const AudioBufferList *input,uint64_t ticks) {
    if(!q || !input || input->mNumberBuffers==0)return;
    uint32_t channels=0,frames=UINT32_MAX;
    for(uint32_t b=0;b<input->mNumberBuffers;b++) {
        const AudioBuffer *buffer=&input->mBuffers[b];
        if(buffer->mNumberChannels==0)continue;
        uint32_t n=buffer->mDataByteSize/(sizeof(float)*buffer->mNumberChannels);
        if(n<frames)frames=n;
        channels+=buffer->mNumberChannels;
    }
    if(frames==0 || frames==UINT32_MAX)return;
    uint64_t first=q->first_frame; q->first_frame+=frames;
    uint64_t time=elapsed(q,ticks),end=time+((uint64_t)frames*1000000000ull)/q->rate;
    uint64_t write=atomic_load_explicit(&q->write_index,memory_order_relaxed);
    uint64_t read=atomic_load_explicit(&q->read_index,memory_order_acquire);
    if(frames>q->max_frames || channels<q->mic_channels+q->system_channels || write-read>=q->capacity) {
        if(q->dropped_frames==0)q->gap_start_ns=time;
        q->gap_end_ns=end; q->dropped_frames+=frames; return;
    }
    VTSlot *slot=&q->slots[write%q->capacity];
    slot->info=(VTFrameInfo){q->epoch,q->rate,q->mic_channels,q->system_channels,frames,first,time,q->dropped_frames,q->gap_start_ns,q->gap_end_ns};
    uint32_t offset=0,total=q->mic_channels+q->system_channels;
    for(uint32_t b=0;b<input->mNumberBuffers && offset<total;b++) {
        const AudioBuffer *buffer=&input->mBuffers[b];
        uint32_t available=buffer->mNumberChannels;
        uint32_t take=available<total-offset?available:total-offset;
        const float *source=(const float *)buffer->mData;
        for(uint32_t f=0;f<frames;f++)for(uint32_t c=0;c<take;c++)
            slot->samples[(size_t)f*total+offset+c]=source?source[(size_t)f*available+c]:0.0f;
        offset+=take;
    }
    q->dropped_frames=0; q->gap_start_ns=0; q->gap_end_ns=0;
    atomic_store_explicit(&q->write_index,write+1,memory_order_release);
}
bool vt_ring_peek(VTAudioRing *q,VTFrameInfo *info) {
    uint64_t read=atomic_load_explicit(&q->read_index,memory_order_relaxed);
    if(read==atomic_load_explicit(&q->write_index,memory_order_acquire))return false;
    *info=q->slots[read%q->capacity].info; return true;
}
void vt_ring_copy_track(VTAudioRing *q,uint32_t track,float *out) {
    uint64_t read=atomic_load_explicit(&q->read_index,memory_order_relaxed);
    VTSlot *slot=&q->slots[read%q->capacity];
    uint32_t channels=track==0?q->mic_channels:q->system_channels;
    uint32_t offset=track==0?0:q->mic_channels,total=q->mic_channels+q->system_channels;
    for(uint32_t f=0;f<slot->info.frame_count;f++)for(uint32_t c=0;c<channels;c++)
        out[(size_t)f*channels+c]=slot->samples[(size_t)f*total+offset+c];
}
void vt_ring_pop(VTAudioRing *q) {
    uint64_t read=atomic_load_explicit(&q->read_index,memory_order_relaxed);
    atomic_store_explicit(&q->read_index,read+1,memory_order_release);
}
uint64_t vt_ring_queued(VTAudioRing *q) {
    if(!q)return 0;
    uint64_t read=atomic_load_explicit(&q->read_index,memory_order_relaxed);
    return atomic_load_explicit(&q->write_index,memory_order_acquire)-read;
}
bool vt_ring_final_gap(VTAudioRing *q,VTFrameInfo *info) {
    if(q->dropped_frames==0)return false;
    *info=(VTFrameInfo){q->epoch,q->rate,q->mic_channels,q->system_channels,0,q->first_frame,q->gap_end_ns,q->dropped_frames,q->gap_start_ns,q->gap_end_ns};
    q->dropped_frames=0;return true;
}
