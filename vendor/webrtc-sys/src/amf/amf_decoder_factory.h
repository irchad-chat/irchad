// irchad: AMD AMF hardware decoding on Windows (vendor/README.md).

#ifndef AMF_DECODER_FACTORY_H_
#define AMF_DECODER_FACTORY_H_

#include <memory>
#include <vector>

#include "api/video_codecs/sdp_video_format.h"
#include "api/video_codecs/video_decoder_factory.h"

namespace webrtc {

class AmfVideoDecoderFactory : public VideoDecoderFactory {
 public:
  AmfVideoDecoderFactory();

  static bool IsSupported();
  static bool IsAv1Supported();

  std::unique_ptr<VideoDecoder> Create(const Environment& env,
                                       const SdpVideoFormat& format) override;
  std::vector<SdpVideoFormat> GetSupportedFormats() const override;

 private:
  std::vector<SdpVideoFormat> supported_formats_;
};

}  // namespace webrtc

#endif  // AMF_DECODER_FACTORY_H_
