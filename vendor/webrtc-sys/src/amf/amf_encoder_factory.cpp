// irchad: AMD AMF hardware encoding on Windows (vendor/README.md).

#include "amf_encoder_factory.h"

#include <map>
#include <string>

#include "absl/container/inlined_vector.h"
#include "amf_encoder.h"
#include "amf_runtime.h"
#include "api/video_codecs/scalability_mode.h"
#include "rtc_base/logging.h"

namespace webrtc {

namespace {

struct AmfProbe {
  bool h264 = false;
  bool av1 = false;
};

AmfProbe ProbeAmf() {
  AmfProbe probe;
  amf::AMFContextPtr context = livekit_ffi::CreateAmfContext();
  if (!context) {
    return probe;
  }

  AmfEncoderParams params;
  params.fps = 30;
  params.bitrate_bps = 2'000'000;
  params.width = 1280;
  params.height = 720;
  if (amf::AMFComponentPtr encoder =
          OpenAmfEncoder(context, AmfCodec::kH264, params)) {
    probe.h264 = true;
    encoder->Terminate();
  }

  // A width that fits neither 64x16 nor 8x2, the two paddings a Radeon AV1
  // encoder may need: if this comes out exact, any even size does.
  params.width = 1282;
  params.height = 722;
  if (amf::AMFComponentPtr encoder =
          OpenAmfEncoder(context, AmfCodec::kAv1, params)) {
    probe.av1 = true;
    encoder->Terminate();
  }

  context->Terminate();
  return probe;
}

const AmfProbe& CachedAmfProbe() {
  static const AmfProbe probe = [] {
    const AmfProbe result = ProbeAmf();
    RTC_LOG(LS_INFO) << "AMD AMF hardware encoder: H264 "
                     << (result.h264 ? "available" : "unavailable") << ", AV1 "
                     << (result.av1 ? "available" : "unavailable") << ".";
    return result;
  }();
  return probe;
}

}  // namespace

AmfVideoEncoderFactory::AmfVideoEncoderFactory() {
  // The same one H.264 format the NVENC factory offers.
  std::map<std::string, std::string> baseline = {
      {"profile-level-id", "42e01f"},
      {"level-asymmetry-allowed", "1"},
      {"packetization-mode", "1"},
  };
  supported_formats_.push_back(SdpVideoFormat("H264", baseline));

  if (IsAv1Supported()) {
    absl::InlinedVector<ScalabilityMode, kScalabilityModeCount> modes;
    modes.push_back(ScalabilityMode::kL1T1);
    supported_formats_.push_back(
        SdpVideoFormat(SdpVideoFormat::AV1Profile0(), modes));
  }
}

bool AmfVideoEncoderFactory::IsSupported() {
  return CachedAmfProbe().h264;
}

bool AmfVideoEncoderFactory::IsAv1Supported() {
  return CachedAmfProbe().h264 && CachedAmfProbe().av1;
}

std::unique_ptr<VideoEncoder> AmfVideoEncoderFactory::Create(
    const Environment& env,
    const SdpVideoFormat& format) {
  for (const SdpVideoFormat& supported : supported_formats_) {
    if (!format.IsSameCodec(supported)) {
      continue;
    }
    const AmfCodec codec =
        format.name == "AV1" ? AmfCodec::kAv1 : AmfCodec::kH264;
    RTC_LOG(LS_INFO) << "Using AMD HW encoder (AMF) for " << format.name;
    return std::make_unique<AmfVideoEncoder>(env, codec, format);
  }
  return nullptr;
}

std::vector<SdpVideoFormat> AmfVideoEncoderFactory::GetSupportedFormats()
    const {
  return supported_formats_;
}

std::vector<SdpVideoFormat> AmfVideoEncoderFactory::GetImplementations()
    const {
  return supported_formats_;
}

}  // namespace webrtc
