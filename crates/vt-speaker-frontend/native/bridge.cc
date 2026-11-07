#include "bridge.h"
#include <algorithm>
#include <cmath>
#include <memory>
#include <vector>
#include "sherpa-onnx/csrc/features.h"
#include "sherpa-onnx/csrc/online-stream.h"
#include "sherpa-onnx/csrc/speaker-embedding-extractor.h"
namespace {
constexpr int32_t kRate = 16000;
constexpr int32_t kDimensions = 256;
constexpr int32_t kBins = 80;
constexpr int32_t kMaxFrames = 498;
constexpr int32_t kOk = 0;
constexpr int32_t kInvalid = 1;
constexpr int32_t kNative = 2;
constexpr int32_t kNotReady = 3;

bool ValidAudio(const float* samples, int32_t count) noexcept {
  if (!samples || count < kRate || count > kRate * 5) return false;
  for (int32_t i = 0; i < count; ++i) {
    if (!std::isfinite(samples[i]) || samples[i] < -1.0f || samples[i] > 1.0f) return false;
  }
  return true;
}
sherpa_onnx::FeatureExtractorConfig ReferenceConfig() {
  sherpa_onnx::FeatureExtractorConfig config;
  config.sampling_rate = kRate;
  config.feature_dim = kBins;
  config.low_freq = 20.0f;
  config.high_freq = 0.0f;
  config.dither = 0.0f;
  config.normalize_samples = false;
  config.snip_edges = true;
  config.frame_shift_ms = 10.0f;
  config.frame_length_ms = 25.0f;
  config.is_librosa = false;
  config.remove_dc_offset = true;
  config.preemph_coeff = 0.97f;
  config.window_type = "hamming";
  config.nemo_normalize_type.clear();
  config.is_mfcc = false;
  config.use_energy = false;
  config.is_whisper = false;
  config.is_t_one = false;
  config.round_to_power_of_two = true;
  // This selects the official non-MFCC Fbank implementation. Its power spectrum
  // is the pinned native default; no alternate FFT or graph is substituted.
  return config;
}
}
struct VtReferenceHandle {
  explicit VtReferenceHandle(const sherpa_onnx::SpeakerEmbeddingExtractorConfig& config)
      : extractor(config) {}
  sherpa_onnx::SpeakerEmbeddingExtractor extractor;
};
extern "C" int32_t vt_reference_create(const char* model, VtReferenceHandle** output) noexcept {
  if (!output) return kInvalid;
  *output = nullptr;
  if (!model || !*model) return kInvalid;
  try {
    sherpa_onnx::SpeakerEmbeddingExtractorConfig config(model, 1, false, "cpu");
    if (!config.Validate()) return kInvalid;
    auto handle = std::make_unique<VtReferenceHandle>(config);
    if (handle->extractor.Dim() != kDimensions) return kNative;
    *output = handle.release();
    return kOk;
  } catch (...) { return kNative; }
}
extern "C" void vt_reference_destroy(VtReferenceHandle* handle) noexcept {
  try { delete handle; } catch (...) { /* no exception crosses the C ABI */ }
}
extern "C" int32_t vt_reference_extract(VtReferenceHandle* handle, const float* samples,
                                        int32_t count, float* output, int32_t capacity) noexcept {
  if (!handle || !output || capacity != kDimensions || !ValidAudio(samples, count)) return kInvalid;
  try {
    sherpa_onnx::OnlineStream stream(ReferenceConfig());
    stream.AcceptWaveform(kRate, samples, count);
    stream.InputFinished();
    if (!handle->extractor.IsReady(&stream)) return kNotReady;
    const auto embedding = handle->extractor.Compute(&stream);
    if (embedding.size() != kDimensions ||
        !std::all_of(embedding.begin(), embedding.end(), [](float v) { return std::isfinite(v); })) return kNative;
    std::copy(embedding.begin(), embedding.end(), output);
    return kOk;
  } catch (...) { return kNative; }
}
extern "C" int32_t vt_reference_features(const float* samples, int32_t count, float* output,
                                         int32_t capacity, int32_t* frames, int32_t* bins) noexcept {
  if (!frames || !bins) return kInvalid;
  *frames = 0;
  *bins = 0;
  if (!output || capacity < kMaxFrames * kBins || !ValidAudio(samples, count)) return kInvalid;
  try {
    sherpa_onnx::OnlineStream stream(ReferenceConfig());
    stream.AcceptWaveform(kRate, samples, count);
    stream.InputFinished();
    const int32_t n = stream.NumFramesReady();
    if (n != 1 + (count - 400) / 160 || n < 1 || n > kMaxFrames || stream.FeatureDim() != kBins) return kNative;
    const auto values = stream.GetFrames(0, n);
    if (values.size() != static_cast<size_t>(n * kBins) ||
        !std::all_of(values.begin(), values.end(), [](float v) { return std::isfinite(v); })) return kNative;
    std::copy(values.begin(), values.end(), output);
    *frames = n;
    *bins = kBins;
    return kOk;
  } catch (...) { return kNative; }
}
