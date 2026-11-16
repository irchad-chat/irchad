// irchad: AMD AMF hardware decoding on Windows (vendor/README.md).

#include "amf_decoder.h"

#include <algorithm>
#include <chrono>
#include <cstring>
#include <optional>
#include <thread>

#include "amf_runtime.h"
#include "api/video/i420_buffer.h"
#include "components/VideoDecoderUVD.h"
#include "modules/video_coding/include/video_error_codes.h"
#include "rtc_base/logging.h"
#include "third_party/libyuv/include/libyuv/convert.h"

namespace webrtc {

namespace {

// How long Decode waits for its frame. Past this it comes out with a later
// one, which is late but still labelled right.
constexpr int kDecodeWaitMs = 25;
// Bounds `pending_` should the decoder stop producing output.
constexpr size_t kMaxPending = 64;

std::optional<uint8_t> QpAsByte(const std::optional<int>& qp) {
  if (!qp || *qp < 0 || *qp > 255) {
    return std::nullopt;
  }
  return static_cast<uint8_t>(*qp);
}

}  // namespace

amf::AMFComponentPtr OpenAmfDecoder(amf::AMFContext* context, AmfCodec codec) {
  const bool av1 = codec == AmfCodec::kAv1;
  const char* name = av1 ? "AV1" : "H264";
  amf::AMFFactory* factory = livekit_ffi::AmfFactory();
  if (!factory || !context) {
    return nullptr;
  }

  amf::AMFComponentPtr decoder;
  const AMF_RESULT res = factory->CreateComponent(
      context, av1 ? AMFVideoDecoderHW_AV1 : AMFVideoDecoderUVD_H264_AVC,
      &decoder);
  if (res != AMF_OK || !decoder) {
    RTC_LOG(LS_INFO) << "AMF has no " << name << " decoder (" << res << ").";
    return nullptr;
  }

  // A preference, not a requirement: nothing that arrives over WebRTC
  // reorders, and a decoder that keeps frames for a DPB only adds latency.
  decoder->SetProperty(AMF_VIDEO_DECODER_REORDER_MODE,
                       amf_int64{AMF_VIDEO_DECODER_MODE_LOW_LATENCY});

  // The size comes from the bitstream; the SDP's is only a maximum.
  const AMF_RESULT init = decoder->Init(amf::AMF_SURFACE_UNKNOWN, 0, 0);
  if (init != AMF_OK) {
    RTC_LOG(LS_WARNING) << "AMF " << name << " decoder failed to start ("
                        << init << ").";
    decoder->Terminate();
    return nullptr;
  }
  return decoder;
}

AmfVideoDecoder::AmfVideoDecoder(AmfCodec codec,
                                 const SdpVideoFormat& /*format*/)
    : codec_(codec) {}

AmfVideoDecoder::~AmfVideoDecoder() {
  Release();
}

bool AmfVideoDecoder::Configure(const Settings& settings) {
  const VideoCodecType expected =
      codec_ == AmfCodec::kAv1 ? kVideoCodecAV1 : kVideoCodecH264;
  if (settings.codec_type() != expected) {
    RTC_LOG(LS_ERROR) << "AMF decoder configured for the wrong codec.";
    return false;
  }
  Release();

  context_ = livekit_ffi::CreateAmfContext();
  if (!context_) {
    RTC_LOG(LS_ERROR) << "AMF: no context on an AMD adapter.";
    return false;
  }
  decoder_ = OpenAmfDecoder(context_, codec_);
  if (!decoder_) {
    Release();
    return false;
  }
  RTC_LOG(LS_INFO) << "AMF " << (codec_ == AmfCodec::kAv1 ? "AV1" : "H264")
                   << " decoder started.";
  return true;
}

int32_t AmfVideoDecoder::RegisterDecodeCompleteCallback(
    DecodedImageCallback* callback) {
  callback_ = callback;
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t AmfVideoDecoder::Release() {
  if (decoder_) {
    decoder_->Terminate();
    decoder_.Release();
  }
  if (context_) {
    context_->Terminate();
    context_.Release();
  }
  pending_.clear();
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t AmfVideoDecoder::Decode(const EncodedImage& input_image,
                                bool /*missing_frames*/,
                                int64_t /*render_time_ms*/) {
  if (!decoder_) {
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }
  if (!callback_) {
    RTC_LOG(LS_ERROR) << "AMF: no decode callback set.";
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }
  if (!input_image.data() || input_image.size() == 0) {
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }

  // The qp a decoded frame is reported with, for the receive stats. The
  // H.264 slice headers say it; nothing in AV1's bitstream does, cheaply.
  std::optional<uint8_t> qp;
  if (codec_ == AmfCodec::kH264) {
    h264_parser_.ParseBitstream(input_image);
    qp = QpAsByte(h264_parser_.GetLastSliceQp());
  }

  amf::AMFBufferPtr buffer;
  if (context_->AllocBuffer(amf::AMF_MEMORY_HOST, input_image.size(),
                            &buffer) != AMF_OK ||
      !buffer || !buffer->GetNative()) {
    return WEBRTC_VIDEO_CODEC_ERROR;
  }
  std::memcpy(buffer->GetNative(), input_image.data(), input_image.size());

  const int64_t pts = next_pts_++;
  buffer->SetPts(pts);
  AMF_RESULT res = decoder_->SubmitInput(buffer);
  if (res == AMF_INPUT_FULL) {
    Drain(-1, kDecodeWaitMs);
    res = decoder_->SubmitInput(buffer);
  }
  if (res == AMF_INPUT_FULL) {
    // Dropped. The frames after this one may not decode; WebRTC asks the
    // sender for a key frame when they don't.
    RTC_LOG(LS_WARNING) << "AMF: decoder is backed up; dropping a frame.";
    return WEBRTC_VIDEO_CODEC_OK;
  }
  if (res != AMF_OK && res != AMF_NEED_MORE_INPUT) {
    RTC_LOG(LS_ERROR) << "AMF: SubmitInput failed (" << res << ").";
    return WEBRTC_VIDEO_CODEC_ERROR;
  }

  if (pending_.size() >= kMaxPending) {
    pending_.pop_front();
  }
  pending_.push_back({pts, input_image.RtpTimestamp(), qp});
  return Drain(pts, kDecodeWaitMs);
}

int32_t AmfVideoDecoder::Drain(int64_t until_pts, int wait_ms) {
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::milliseconds(wait_ms);
  for (;;) {
    amf::AMFDataPtr data;
    const AMF_RESULT res = decoder_->QueryOutput(&data);
    if (data) {
      amf::AMFSurfacePtr surface(data);
      if (!surface || pending_.empty()) {
        continue;
      }
      // Output keeps input order, so frames queued before this one made
      // nothing. A pts we never set means the driver didn't carry it over:
      // take the oldest frame.
      const int64_t pts = data->GetPts();
      auto match = std::find_if(pending_.begin(), pending_.end(),
                                [pts](const Pending& p) { return p.pts == pts; });
      if (match == pending_.end()) {
        match = pending_.begin();
      }
      const Pending frame = *match;
      pending_.erase(pending_.begin(), match + 1);
      Deliver(surface, frame);
      if (frame.pts == until_pts) {
        return WEBRTC_VIDEO_CODEC_OK;
      }
      continue;
    }
    if (res != AMF_OK && res != AMF_REPEAT && res != AMF_NEED_MORE_INPUT) {
      RTC_LOG(LS_ERROR) << "AMF: QueryOutput failed (" << res << ").";
      return WEBRTC_VIDEO_CODEC_ERROR;
    }
    if (std::chrono::steady_clock::now() >= deadline) {
      return WEBRTC_VIDEO_CODEC_OK;
    }
    // Only reached quickly on drivers without a query timeout.
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
}

void AmfVideoDecoder::Deliver(amf::AMFSurface* surface, const Pending& frame) {
  // The decode happened on the GPU; this is the copy back out.
  if (surface->GetMemoryType() != amf::AMF_MEMORY_HOST &&
      surface->Convert(amf::AMF_MEMORY_HOST) != AMF_OK) {
    RTC_LOG(LS_ERROR) << "AMF: couldn't download the decoded frame.";
    return;
  }
  amf::AMFPlane* y = surface->GetPlane(amf::AMF_PLANE_Y);
  amf::AMFPlane* uv = surface->GetPlane(amf::AMF_PLANE_UV);
  if (!y || !uv || !y->GetNative() || !uv->GetNative()) {
    return;
  }
  const int width = y->GetWidth();
  const int height = y->GetHeight();
  if (width <= 0 || height <= 0 || y->GetHPitch() < width) {
    return;
  }
  scoped_refptr<I420Buffer> i420 = buffer_pool_.CreateI420Buffer(width, height);
  if (!i420) {
    RTC_LOG(LS_ERROR) << "AMF: out of buffers; dropping a frame.";
    return;
  }
  if (libyuv::NV12ToI420(
          static_cast<const uint8_t*>(y->GetNative()), y->GetHPitch(),
          static_cast<const uint8_t*>(uv->GetNative()), uv->GetHPitch(),
          i420->MutableDataY(), i420->StrideY(), i420->MutableDataU(),
          i420->StrideU(), i420->MutableDataV(), i420->StrideV(), width,
          height) != 0) {
    RTC_LOG(LS_ERROR) << "AMF: NV12ToI420 failed.";
    return;
  }

  VideoFrame decoded = VideoFrame::Builder()
                           .set_video_frame_buffer(i420)
                           .set_timestamp_rtp(frame.rtp_timestamp)
                           .build();
  callback_->Decoded(decoded, std::nullopt, frame.qp);
}

VideoDecoder::DecoderInfo AmfVideoDecoder::GetDecoderInfo() const {
  DecoderInfo info;
  info.implementation_name =
      codec_ == AmfCodec::kAv1 ? "AMD AMF AV1 Decoder" : "AMD AMF H264 Decoder";
  info.is_hardware_accelerated = true;
  return info;
}

}  // namespace webrtc
