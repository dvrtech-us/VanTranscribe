#ifndef VT_SPEAKER_FRONTEND_BRIDGE_H_
#define VT_SPEAKER_FRONTEND_BRIDGE_H_
#include <cstdint>
// This is our owned C bridge container, never a cast of a sherpa C-API object.
struct VtReferenceHandle;
extern "C" {
int32_t vt_reference_create(const char* model, VtReferenceHandle** output) noexcept;
void vt_reference_destroy(VtReferenceHandle* handle) noexcept;
int32_t vt_reference_extract(VtReferenceHandle* handle, const float* samples,
                             int32_t count, float* output, int32_t capacity) noexcept;
int32_t vt_reference_features(const float* samples, int32_t count, float* output,
                              int32_t capacity, int32_t* frames, int32_t* bins) noexcept;
}
#endif
