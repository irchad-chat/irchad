// irchad: AMD AMF hardware decoding on Windows (vendor/README.md).

#ifndef AMF_DECODER_H_
#define AMF_DECODER_H_

#include <cstdint>
#include <deque>
#include <optional>

#include "amf_encoder.h"  // AmfCodec
#include "api/video_codecs/video_decoder.h"
#include "common_video/h264/h264_bitstream_parser.h"
#include "common_video/include/video_frame_buffer_pool.h"
#include "components/Component.h"
#include "core/Context.h"

namespace webrtc {

// A configured, initialised AMF decoder, or null: the size comes from the
// bitstream, not the SDP. Used for the probe and for real decoders, so the
// two can't disagree.
amf::AMFComponentPtr OpenAmfDecoder(amf::AMFContext* context, AmfCodec codec);

// H.264 or AV1 on a Radeon's decoder. Encoded images reach AMF as host
// buffers (Annex B for H.264, low-overhead OBUs for AV1); the decoded NV12
// comes back through a host copy and one libyuv pass.
class AmfVideoDecoder : public VideoDecoder {
 public:
  AmfVideoDecoder(AmfCodec codec, const SdpVideoFormat& format);
  ~AmfVideoDecoder() override;

  bool Configure(const Settings& settings) override;
  int32_t RegisterDecodeCompleteCallback(DecodedImageCallback* callback) override;
  int32_t Release() override;
  int32_t Decode(const EncodedImage& input_image,
                 bool missing_frames,
                 int64_t render_time_ms) override;
  DecoderInfo GetDecoderInfo() const override;

 private:
  // What an output surface needs from the frame it decodes. AMF copies the
  // input's pts to the output, which is how the two are matched.
  struct Pending {
    int64_t pts;
    uint32_t rtp_timestamp;
    std::optional<uint8_t> qp;
  };

  // Delivers every output AMF has ready, waiting up to `wait_ms` for the one
  // with `until_pts`.
  int32_t Drain(int64_t until_pts, int wait_ms);
  void Deliver(amf::AMFSurface* surface, const Pending& frame);

  const AmfCodec codec_;

  DecodedImageCallback* callback_ = nullptr;
  amf::AMFContextPtr context_;
  amf::AMFComponentPtr decoder_;

  int64_t next_pts_ = 0;
  std::deque<Pending> pending_;
  VideoFrameBufferPool buffer_pool_;
  H264BitstreamParser h264_parser_;
};

}  // namespace webrtc

#endif  // AMF_DECODER_H_
