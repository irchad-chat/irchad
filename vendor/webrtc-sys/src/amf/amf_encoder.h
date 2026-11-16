// irchad: AMD AMF hardware encoding on Windows (vendor/README.md).

#ifndef AMF_ENCODER_H_
#define AMF_ENCODER_H_

#include <cstdint>
#include <deque>
#include <optional>
#include <vector>

#include "api/environment/environment.h"
#include "api/video/color_space.h"
#include "api/video/video_rotation.h"
#include "api/video_codecs/h264_profile_level_id.h"
#include "api/video_codecs/sdp_video_format.h"
#include "api/video_codecs/video_encoder.h"
#include "common_video/h264/h264_bitstream_parser.h"
#include "components/Component.h"
#include "core/Context.h"
#include "modules/video_coding/svc/scalable_video_controller_no_layering.h"

namespace webrtc {

enum class AmfCodec { kH264, kAv1 };

struct AmfEncoderParams {
  int width = 0;
  int height = 0;
  uint32_t fps = 30;
  uint32_t bitrate_bps = 0;
  H264Profile h264_profile = H264Profile::kProfileConstrainedBaseline;
};

// A configured, initialised AMF encoder, or null. An AV1 encoder is only
// returned when its stream is exactly `width` x `height`: some Radeon AV1
// encoders code a padded picture (64x16 or 8x2 aligned), and nothing in RTP
// tells the viewer to crop it. Used for the probe and for real encoders, so
// the two can't disagree.
amf::AMFComponentPtr OpenAmfEncoder(amf::AMFContext* context,
                                    AmfCodec codec,
                                    const AmfEncoderParams& params);

// The picture size an AV1 sequence header OBU declares.
bool Av1SequenceHeaderSize(const uint8_t* obu,
                           size_t len,
                           int* width,
                           int* height);

// H.264 or AV1 on a Radeon's encoder: one frame in, one frame out, no
// B-frames, constant bitrate. Frames reach AMF as NV12 in host memory, which
// AMF uploads to the GPU.
class AmfVideoEncoder : public VideoEncoder {
 public:
  AmfVideoEncoder(const Environment& env,
                  AmfCodec codec,
                  const SdpVideoFormat& format);
  ~AmfVideoEncoder() override;

  int32_t InitEncode(const VideoCodec* codec_settings,
                     const Settings& settings) override;
  int32_t RegisterEncodeCompleteCallback(
      EncodedImageCallback* callback) override;
  int32_t Release() override;
  int32_t Encode(const VideoFrame& frame,
                 const std::vector<VideoFrameType>* frame_types) override;
  void SetRates(const RateControlParameters& parameters) override;
  EncoderInfo GetEncoderInfo() const override;

 private:
  // What an output buffer needs from the frame it was encoded from. AMF copies
  // the input's pts to the output, which is how the two are matched.
  struct Pending {
    int64_t pts;
    uint32_t rtp_timestamp;
    int64_t ntp_time_ms;
    int64_t capture_time_ms;
    VideoRotation rotation;
    std::optional<ColorSpace> color_space;
  };

  // Delivers every output AMF has ready, waiting up to `wait_ms` for the one
  // with `until_pts`.
  int32_t Drain(int64_t until_pts, int wait_ms);
  void Deliver(amf::AMFBuffer* buffer, const Pending& frame);
  void ApplyRates(bool with_fps);

  const AmfCodec codec_;
  H264Profile h264_profile_ = H264Profile::kProfileConstrainedBaseline;

  EncodedImageCallback* callback_ = nullptr;
  amf::AMFContextPtr context_;
  amf::AMFComponentPtr encoder_;

  int width_ = 0;
  int height_ = 0;
  uint32_t fps_ = 30;
  uint32_t bitrate_bps_ = 0;
  bool sending_ = false;
  bool key_frame_request_ = true;
  int64_t next_pts_ = 0;
  std::deque<Pending> pending_;

  // SPS and PPS, or the AV1 sequence header: put back on a key frame that
  // came out without them, which no receiver could start from.
  std::vector<uint8_t> headers_;
  H264BitstreamParser h264_parser_;
  ScalableVideoControllerNoLayering svc_controller_;
};

}  // namespace webrtc

#endif  // AMF_ENCODER_H_
