// irchad: AMD AMF hardware decoding on Windows (vendor/README.md).

#include "amf_decoder_factory.h"

#include "amf_decoder.h"
#include "amf_runtime.h"
#include "modules/video_coding/codecs/h264/include/h264.h"
#include "rtc_base/logging.h"

namespace webrtc {

namespace {

struct AmfDecoderProbe {
  bool h264 = false;
  bool av1 = false;
};

AmfDecoderProbe ProbeAmfDecoder() {
  AmfDecoderProbe probe;
  amf::AMFContextPtr context = livekit_ffi::CreateAmfContext();
  if (!context) {
    return probe;
  }

  if (amf::AMFComponentPtr decoder =
          OpenAmfDecoder(context, AmfCodec::kH264)) {
    probe.h264 = true;
    decoder->Terminate();
  }
  if (amf::AMFComponentPtr decoder = OpenAmfDecoder(context, AmfCodec::kAv1)) {
    probe.av1 = true;
    decoder->Terminate();
  }

  context->Terminate();
  return probe;
}

const AmfDecoderProbe& CachedAmfDecoderProbe() {
  static const AmfDecoderProbe probe = [] {
    const AmfDecoderProbe result = ProbeAmfDecoder();
    RTC_LOG(LS_INFO) << "AMD AMF hardware decoder: H264 "
                     << (result.h264 ? "available" : "unavailable")
                     << ", AV1 " << (result.av1 ? "available" : "unavailable")
                     << ".";
    return result;
  }();
  return probe;
}

}  // namespace

AmfVideoDecoderFactory::AmfVideoDecoderFactory() {
  // The same H.264 formats the NVDEC factory offers: whatever of these the
  // SFU sends, the Radeon's decoder takes.
  supported_formats_ = {
      CreateH264Format(H264Profile::kProfileConstrainedBaseline,
                       H264Level::kLevel5_1, "1"),
      CreateH264Format(H264Profile::kProfileBaseline, H264Level::kLevel5_1, "1"),
      CreateH264Format(H264Profile::kProfileHigh, H264Level::kLevel5_1, "1"),
      CreateH264Format(H264Profile::kProfileMain, H264Level::kLevel5_1, "1"),
  };

  if (IsAv1Supported()) {
    supported_formats_.push_back(SdpVideoFormat(SdpVideoFormat::AV1Profile0()));
  }
}

bool AmfVideoDecoderFactory::IsSupported() {
  return CachedAmfDecoderProbe().h264;
}

bool AmfVideoDecoderFactory::IsAv1Supported() {
  return CachedAmfDecoderProbe().h264 && CachedAmfDecoderProbe().av1;
}

std::unique_ptr<VideoDecoder> AmfVideoDecoderFactory::Create(
    const Environment& env,
    const SdpVideoFormat& format) {
  for (const SdpVideoFormat& supported : supported_formats_) {
    if (!format.IsSameCodec(supported)) {
      continue;
    }
    const AmfCodec codec = format.name == "AV1" ? AmfCodec::kAv1 : AmfCodec::kH264;
    RTC_LOG(LS_INFO) << "Using AMD HW decoder (AMF) for " << format.name;
    return std::make_unique<AmfVideoDecoder>(codec, format);
  }
  return nullptr;
}

std::vector<SdpVideoFormat> AmfVideoDecoderFactory::GetSupportedFormats()
    const {
  return supported_formats_;
}

}  // namespace webrtc
