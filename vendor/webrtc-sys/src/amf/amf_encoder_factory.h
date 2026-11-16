// irchad: AMD AMF hardware encoding on Windows (vendor/README.md).

#ifndef AMF_ENCODER_FACTORY_H_
#define AMF_ENCODER_FACTORY_H_

#include <memory>
#include <vector>

#include "api/environment/environment.h"
#include "api/video_codecs/sdp_video_format.h"
#include "api/video_codecs/video_encoder_factory.h"

namespace webrtc {

class AmfVideoEncoderFactory : public VideoEncoderFactory {
 public:
  AmfVideoEncoderFactory();

  // Whether a Radeon here encodes H.264. Opens a real encoder once, on first
  // use, and remembers the answer.
  static bool IsSupported();
  // Whether it also encodes AV1 at any (even) size, as screen share needs.
  static bool IsAv1Supported();

  std::unique_ptr<VideoEncoder> Create(const Environment& env,
                                       const SdpVideoFormat& format) override;
  std::vector<SdpVideoFormat> GetSupportedFormats() const override;
  std::vector<SdpVideoFormat> GetImplementations() const override;

 private:
  std::vector<SdpVideoFormat> supported_formats_;
};

}  // namespace webrtc

#endif  // AMF_ENCODER_FACTORY_H_
