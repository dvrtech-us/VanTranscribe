#include "AudioRing.h"
#include <assert.h>
#include <mach/mach_time.h>
#include <stdlib.h>
#include <stdio.h>

static void interleaved_tracks_and_overflow(void) {
    uint64_t origin=mach_absolute_time();
    VTAudioRing *q=vt_ring_create(8000,1,2,8,3,origin);
    assert(q);
    float input[24]; for(unsigned f=0;f<8;f++){input[f*3]=0.25f;input[f*3+1]=0.5f;input[f*3+2]=-0.75f;}
    AudioBufferList list={.mNumberBuffers=1,.mBuffers={{.mNumberChannels=3,.mDataByteSize=sizeof(input),.mData=input}}};
    // 10 s / 8-frame quantum + two reserve slots.
    for(unsigned n=0;n<10005;n++)vt_ring_write(q,&list,origin+n*1000);
    VTFrameInfo info; assert(vt_ring_peek(q,&info));
    assert(info.epoch==3 && info.first_frame==0 && info.frame_count==8);
    float mic[8],system[16]; vt_ring_copy_track(q,0,mic);vt_ring_copy_track(q,1,system);
    for(unsigned f=0;f<8;f++){assert(mic[f]==0.25f);assert(system[f*2]==0.5f);assert(system[f*2+1]==-0.75f);}
    vt_ring_pop(q);
    vt_ring_write(q,&list,origin+10005*1000);
    unsigned packets=0;bool gap=false;
    while(vt_ring_peek(q,&info)){packets++;if(info.dropped_frames){gap=true;assert(info.dropped_frames==24);assert(info.first_frame==80040);assert(info.gap_end_ns>info.gap_start_ns);}vt_ring_pop(q);}
    assert(packets==10002 && gap);
    vt_ring_destroy(q);
}

static void planar_input_and_final_gap(void) {
    uint64_t origin=mach_absolute_time();
    VTAudioRing *q=vt_ring_create(8000,1,2,8,0,origin);assert(q);
    AudioBufferList *list=calloc(1,sizeof(AudioBufferList)+2*sizeof(AudioBuffer));
    float mic[9],left[9],right[9];for(unsigned i=0;i<9;i++){mic[i]=1;left[i]=2;right[i]=3;}
    list->mNumberBuffers=3;
    list->mBuffers[0]=(AudioBuffer){1,8*sizeof(float),mic};
    list->mBuffers[1]=(AudioBuffer){1,8*sizeof(float),left};
    list->mBuffers[2]=(AudioBuffer){1,8*sizeof(float),right};
    vt_ring_write(q,list,origin);
    float out[16];vt_ring_copy_track(q,1,out);for(unsigned i=0;i<8;i++){assert(out[i*2]==2);assert(out[i*2+1]==3);}vt_ring_pop(q);
    for(unsigned i=0;i<3;i++)list->mBuffers[i].mDataByteSize=9*sizeof(float);
    vt_ring_write(q,list,origin+1000);VTFrameInfo info;assert(!vt_ring_peek(q,&info));assert(vt_ring_final_gap(q,&info));assert(info.dropped_frames==9);assert(!vt_ring_final_gap(q,&info));
    free(list);vt_ring_destroy(q);
}
int main(void) {interleaved_tracks_and_overflow();planar_input_and_final_gap();puts("AudioRing: interleaved/planar separation and overflow accounting passed");}
