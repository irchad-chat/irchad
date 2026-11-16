// irchad: AMD AMF hardware encoding on Windows (vendor/README.md).

#include "amf_encoder.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <optional>
#include <thread>

#include "../av1_bitstream.h"
#include "amf_runtime.h"
#include "api/video/encoded_image.h"
#include "api/video/i420_buffer.h"
#include "api/video_codecs/scalability_mode.h"
#include "common_video/h264/h264_common.h"
#include "components/VideoEncoderAV1.h"
#include "components/VideoEncoderVCE.h"
#include "core/Buffer.h"
#include "core/Surface.h"
#include "modules/video_coding/codecs/h264/include/h264.h"
#include "modules/video_coding/include/video_codec_interface.h"
#include "modules/video_coding/include/video_error_codes.h"
#include "rtc_base/logging.h"
#include "third_party/libyuv/include/libyuv/convert_from.h"

namespace webrtc {

namespace {

// How long QueryOutput may block, where the driver supports that.
constexpr amf_int64 kQueryTimeoutMs = 10;
// How long Encode waits for its frame. Past this the frame comes out with a
// later one, which is late but still labelled right.
constexpr int kEncodeWaitMs = 100;
// Bounds `pending_` should the encoder stop producing output.
constexpr size_t kMaxPending = 64;

template <typename T>
bool Set(amf::AMFPropertyStorage* object, const wchar_t* name, const T& value) {
  return object->SetProperty(name, value) == AMF_OK;
}

amf_int64 AmfH264Profile(H264Profile profile) {
  switch (profile) {
    case H264Profile::kProfileBaseline:
      return AMF_VIDEO_ENCODER_PROFILE_BASELINE;
    case H264Profile::kProfileMain:
      return AMF_VIDEO_ENCODER_PROFILE_MAIN;
    case H264Profile::kProfileHigh:
      return AMF_VIDEO_ENCODER_PROFILE_HIGH;
    case H264Profile::kProfileConstrainedHigh:
      return AMF_VIDEO_ENCODER_PROFILE_CONSTRAINED_HIGH;
    default:
      return AMF_VIDEO_ENCODER_PROFILE_CONSTRAINED_BASELINE;
  }
}

// The lowest level that fits, from 4.2 (AMF's default) up. The level in the
// SDP (3.1 in the usual 42e01f) stops at 720p30, and no software encoder
// honours it either; receivers decode the level the stream carries.
amf_int64 H264Level(int width, int height, uint32_t fps) {
  struct Limit {
    amf_int64 level;
    int64_t max_frame_mbs;
    int64_t max_mbs_per_sec;
  };
  static constexpr Limit kLimits[] = {
      {AMF_H264_LEVEL__4_2, 8704, 522240},
      {AMF_H264_LEVEL__5, 22080, 589824},
      {AMF_H264_LEVEL__5_1, 36864, 983040},
      {AMF_H264_LEVEL__5_2, 36864, 2073600},
      {AMF_H264_LEVEL__6, 139264, 4177920},
      {AMF_H264_LEVEL__6_1, 139264, 8355840},
  };
  const int64_t mbs =
      static_cast<int64_t>((width + 15) / 16) * ((height + 15) / 16);
  for (const Limit& limit : kLimits) {
    if (mbs <= limit.max_frame_mbs && mbs * fps <= limit.max_mbs_per_sec) {
      return limit.level;
    }
  }
  return AMF_H264_LEVEL__6_2;
}

// Each list starts with what the encoder can't run without; the rest are
// preferences an older driver or card may refuse.
bool ConfigureH264(amf::AMFComponent* encoder, const AmfEncoderParams& p) {
  const amf_int64 bitrate = p.bitrate_bps;
  // Usage first: it resets every other property to that usage's defaults.
  if (!Set(encoder, AMF_VIDEO_ENCODER_USAGE,
           amf_int64{AMF_VIDEO_ENCODER_USAGE_LOW_LATENCY}) ||
      !Set(encoder, AMF_VIDEO_ENCODER_PROFILE,
           AmfH264Profile(p.h264_profile)) ||
      !Set(encoder, AMF_VIDEO_ENCODER_FRAMESIZE,
           AMFConstructSize(p.width, p.height)) ||
      !Set(encoder, AMF_VIDEO_ENCODER_FRAMERATE, AMFConstructRate(p.fps, 1)) ||
      !Set(encoder, AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD,
           amf_int64{AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD_CBR}) ||
      !Set(encoder, AMF_VIDEO_ENCODER_TARGET_BITRATE, bitrate) ||
      !Set(encoder, AMF_VIDEO_ENCODER_PEAK_BITRATE, bitrate)) {
    return false;
  }

  amf_int64 level = H264Level(p.width, p.height, p.fps);
  amf::AMFCapsPtr caps;
  amf_int64 max_level = 0;
  if (encoder->GetCaps(&caps) == AMF_OK && caps &&
      caps->GetProperty(AMF_VIDEO_ENCODER_CAP_MAX_LEVEL, &max_level) ==
          AMF_OK &&
      max_level > 0) {
    level = std::min(level, max_level);
  }
  Set(encoder, AMF_VIDEO_ENCODER_PROFILE_LEVEL, level);

  // One second of buffer, as libwebrtc's software encoders use.
  Set(encoder, AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE, bitrate);
  Set(encoder, AMF_VIDEO_ENCODER_ENFORCE_HRD, true);
  Set(encoder, AMF_VIDEO_ENCODER_FILLER_DATA_ENABLE, false);
  Set(encoder, AMF_VIDEO_ENCODER_B_PIC_PATTERN, amf_int64{0});
  // No key frames of its own: WebRTC asks for one when a receiver needs it.
  Set(encoder, AMF_VIDEO_ENCODER_IDR_PERIOD, amf_int64{0});
  Set(encoder, AMF_VIDEO_ENCODER_QUALITY_PRESET,
      amf_int64{AMF_VIDEO_ENCODER_QUALITY_PRESET_BALANCED});
  Set(encoder, AMF_VIDEO_ENCODER_LOWLATENCY_MODE, true);
  Set(encoder, AMF_VIDEO_ENCODER_QUERY_TIMEOUT, kQueryTimeoutMs);
  return true;
}

bool ConfigureAv1(amf::AMFComponent* encoder, const AmfEncoderParams& p) {
  const amf_int64 bitrate = p.bitrate_bps;
  if (!Set(encoder, AMF_VIDEO_ENCODER_AV1_USAGE,
           amf_int64{AMF_VIDEO_ENCODER_AV1_USAGE_LOW_LATENCY}) ||
      !Set(encoder, AMF_VIDEO_ENCODER_AV1_FRAMESIZE,
           AMFConstructSize(p.width, p.height)) ||
      !Set(encoder, AMF_VIDEO_ENCODER_AV1_FRAMERATE,
           AMFConstructRate(p.fps, 1)) ||
      !Set(encoder, AMF_VIDEO_ENCODER_AV1_RATE_CONTROL_METHOD,
           amf_int64{AMF_VIDEO_ENCODER_AV1_RATE_CONTROL_METHOD_CBR}) ||
      !Set(encoder, AMF_VIDEO_ENCODER_AV1_TARGET_BITRATE, bitrate) ||
      !Set(encoder, AMF_VIDEO_ENCODER_AV1_PEAK_BITRATE, bitrate)) {
    return false;
  }

  // Where the card can't do this, OpenAmfEncoder's size check turns AV1 down.
  Set(encoder, AMF_VIDEO_ENCODER_AV1_ALIGNMENT_MODE,
      amf_int64{AMF_VIDEO_ENCODER_AV1_ALIGNMENT_MODE_NO_RESTRICTIONS});
  Set(encoder, AMF_VIDEO_ENCODER_AV1_VBV_BUFFER_SIZE, bitrate);
  Set(encoder, AMF_VIDEO_ENCODER_AV1_ENFORCE_HRD, true);
  Set(encoder, AMF_VIDEO_ENCODER_AV1_FILLER_DATA, false);
  Set(encoder, AMF_VIDEO_ENCODER_AV1_GOP_SIZE, amf_int64{0});
  Set(encoder, AMF_VIDEO_ENCODER_AV1_QUALITY_PRESET,
      amf_int64{AMF_VIDEO_ENCODER_AV1_QUALITY_PRESET_BALANCED});
  Set(encoder, AMF_VIDEO_ENCODER_AV1_QUERY_TIMEOUT, kQueryTimeoutMs);
  return true;
}

// SPS and PPS in Annex B, or the AV1 sequence header OBU. Empty if the
// encoder doesn't hand them out.
std::vector<uint8_t> ExtraData(amf::AMFComponent* encoder, AmfCodec codec) {
  amf::AMFVariant value;
  const wchar_t* name = codec == AmfCodec::kAv1
                            ? AMF_VIDEO_ENCODER_AV1_EXTRA_DATA
                            : AMF_VIDEO_ENCODER_EXTRADATA;
  if (encoder->GetProperty(name,
                           static_cast<amf::AMFVariantStruct*>(&value)) !=
          AMF_OK ||
      !value.ToInterface()) {
    return {};
  }
  amf::AMFBufferPtr buffer(value.ToInterface());
  if (!buffer || !buffer->GetNative() || buffer->GetSize() == 0) {
    return {};
  }
  const uint8_t* data = static_cast<const uint8_t*>(buffer->GetNative());
  std::vector<uint8_t> bytes(data, data + buffer->GetSize());
  if (codec == AmfCodec::kAv1) {
    std::vector<uint8_t> sequence_header;
    if (!livekit::av1::ExtractSequenceHeaderObu(bytes.data(), bytes.size(),
                                                &sequence_header)) {
      return {};
    }
    return sequence_header;
  }
  return bytes;
}

class BitReader {
 public:
  BitReader(const uint8_t* data, size_t len) : data_(data), bits_(len * 8) {}

  uint32_t Read(int count) {
    uint32_t value = 0;
    for (int i = 0; i < count; ++i) {
      if (pos_ >= bits_) {
        ok_ = false;
        return 0;
      }
      value = (value << 1) | ((data_[pos_ >> 3] >> (7 - (pos_ & 7))) & 1);
      ++pos_;
    }
    return value;
  }

  void Skip(size_t count) {
    pos_ += count;
    ok_ = ok_ && pos_ <= bits_;
  }

  bool ok() const { return ok_; }

 private:
  const uint8_t* data_;
  size_t bits_;
  size_t pos_ = 0;
  bool ok_ = true;
};

}  // namespace

// AV1 spec 5.5.1 (sequence_header_obu), read as far as the frame size.
bool Av1SequenceHeaderSize(const uint8_t* obu,
                           size_t len,
                           int* width,
                           int* height) {
  constexpr int kObuSequenceHeader = 1;
  if (!obu || len < 2 || ((obu[0] >> 3) & 0xF) != kObuSequenceHeader) {
    return false;
  }
  const bool has_extension = obu[0] & 0x4;
  const bool has_size = obu[0] & 0x2;
  size_t offset = has_extension ? 2 : 1;
  if (has_size) {
    // leb128: the last byte has its top bit clear.
    while (offset < len && (obu[offset++] & 0x80)) {
    }
  }
  if (offset >= len) {
    return false;
  }

  BitReader r(obu + offset, len - offset);
  r.Skip(3);  // seq_profile
  r.Skip(1);  // still_picture
  if (r.Read(1)) {  // reduced_still_picture_header
    r.Skip(5);      // seq_level_idx[0]
  } else {
    bool decoder_model = false;
    size_t buffer_delay_bits = 0;
    if (r.Read(1)) {  // timing_info_present_flag
      r.Skip(64);     // num_units_in_display_tick, time_scale
      if (r.Read(1)) {  // equal_picture_interval: num_ticks_per_picture, uvlc
        int zeros = 0;
        while (r.ok() && r.Read(1) == 0) {
          if (++zeros > 32) {
            return false;
          }
        }
        if (zeros < 32) {
          r.Skip(zeros);
        }
      }
      decoder_model = r.Read(1);
      if (decoder_model) {
        buffer_delay_bits = r.Read(5) + 1;
        r.Skip(32 + 5 + 5);
      }
    }
    const bool display_delay = r.Read(1);
    const uint32_t operating_points = r.Read(5) + 1;
    for (uint32_t i = 0; i < operating_points; ++i) {
      r.Skip(12);            // operating_point_idc
      if (r.Read(5) > 7) {   // seq_level_idx
        r.Skip(1);           // seq_tier
      }
      if (decoder_model && r.Read(1)) {
        r.Skip(2 * buffer_delay_bits + 1);
      }
      if (display_delay && r.Read(1)) {
        r.Skip(4);
      }
    }
  }
  const int width_bits = r.Read(4) + 1;
  const int height_bits = r.Read(4) + 1;
  *width = static_cast<int>(r.Read(width_bits)) + 1;
  *height = static_cast<int>(r.Read(height_bits)) + 1;
  return r.ok();
}

amf::AMFComponentPtr OpenAmfEncoder(amf::AMFContext* context,
                                    AmfCodec codec,
                                    const AmfEncoderParams& params) {
  const bool av1 = codec == AmfCodec::kAv1;
  const char* name = av1 ? "AV1" : "H264";
  amf::AMFFactory* factory = livekit_ffi::AmfFactory();
  if (!factory || !context) {
    return nullptr;
  }

  amf::AMFComponentPtr encoder;
  AMF_RESULT res = factory->CreateComponent(
      context, av1 ? AMFVideoEncoder_AV1 : AMFVideoEncoderVCE_AVC, &encoder);
  if (res != AMF_OK || !encoder) {
    RTC_LOG(LS_INFO) << "AMF has no " << name << " encoder (" << res << ").";
    return nullptr;
  }

  if (!(av1 ? ConfigureAv1(encoder, params)
            : ConfigureH264(encoder, params))) {
    RTC_LOG(LS_WARNING) << "AMF " << name << " encoder refused its settings.";
    encoder->Terminate();
    return nullptr;
  }
  res = encoder->Init(amf::AMF_SURFACE_NV12, params.width, params.height);
  if (res != AMF_OK) {
    RTC_LOG(LS_WARNING) << "AMF " << name << " encoder failed to start at "
                        << params.width << "x" << params.height << " ("
                        << res << ").";
    encoder->Terminate();
    return nullptr;
  }

  if (av1) {
    const std::vector<uint8_t> header = ExtraData(encoder, codec);
    int width = 0;
    int height = 0;
    if (!Av1SequenceHeaderSize(header.data(), header.size(), &width,
                               &height) ||
        width != params.width || height != params.height) {
      RTC_LOG(LS_WARNING) << "AMF AV1 would code " << params.width << "x"
                          << params.height << " as " << width << "x" << height
                          << "; not using it.";
      encoder->Terminate();
      return nullptr;
    }
  }
  return encoder;
}

AmfVideoEncoder::AmfVideoEncoder(const Environment& /*env*/,
                                 AmfCodec codec,
                                 const SdpVideoFormat& format)
    : codec_(codec) {
  if (codec_ == AmfCodec::kH264) {
    if (auto profile_level_id = ParseSdpForH264ProfileLevelId(format.parameters)) {
      h264_profile_ = profile_level_id->profile;
    }
  }
}

AmfVideoEncoder::~AmfVideoEncoder() {
  Release();
}

int32_t AmfVideoEncoder::InitEncode(const VideoCodec* inst,
                                    const Settings& /*settings*/) {
  const VideoCodecType expected =
      codec_ == AmfCodec::kAv1 ? kVideoCodecAV1 : kVideoCodecH264;
  // 4:2:0 has no odd sizes; GetEncoderInfo asks WebRTC for even ones.
  if (!inst || inst->codecType != expected || inst->maxFramerate == 0 ||
      inst->width < 2 || inst->height < 2 || inst->width % 2 != 0 ||
      inst->height % 2 != 0) {
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  Release();

  width_ = inst->width;
  height_ = inst->height;
  fps_ = inst->maxFramerate;
  bitrate_bps_ = std::max<uint32_t>(inst->startBitrate, 100) * 1000;

  context_ = livekit_ffi::CreateAmfContext();
  if (!context_) {
    RTC_LOG(LS_ERROR) << "AMF: no context on an AMD adapter.";
    return WEBRTC_VIDEO_CODEC_ERROR;
  }
  AmfEncoderParams params;
  params.width = width_;
  params.height = height_;
  params.fps = fps_;
  params.bitrate_bps = bitrate_bps_;
  params.h264_profile = h264_profile_;
  encoder_ = OpenAmfEncoder(context_, codec_, params);
  if (!encoder_) {
    Release();
    return WEBRTC_VIDEO_CODEC_ERROR;
  }

  headers_ = ExtraData(encoder_, codec_);
  svc_controller_ = ScalableVideoControllerNoLayering();
  sending_ = true;
  key_frame_request_ = true;
  next_pts_ = 0;
  RTC_LOG(LS_INFO) << "AMF " << (codec_ == AmfCodec::kAv1 ? "AV1" : "H264")
                   << " encoder started: " << width_ << "x" << height_ << " @ "
                   << fps_ << " fps, " << bitrate_bps_ << " bps";
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t AmfVideoEncoder::RegisterEncodeCompleteCallback(
    EncodedImageCallback* callback) {
  callback_ = callback;
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t AmfVideoEncoder::Release() {
  if (encoder_) {
    encoder_->Terminate();
    encoder_.Release();
  }
  if (context_) {
    context_->Terminate();
    context_.Release();
  }
  pending_.clear();
  headers_.clear();
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t AmfVideoEncoder::Encode(
    const VideoFrame& frame,
    const std::vector<VideoFrameType>* frame_types) {
  if (!encoder_ || !callback_) {
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }
  if (!sending_) {
    return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
  }
  bool key_frame = key_frame_request_;
  if (frame_types && !frame_types->empty()) {
    if ((*frame_types)[0] == VideoFrameType::kEmptyFrame) {
      return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
    }
    key_frame |= (*frame_types)[0] == VideoFrameType::kVideoFrameKey;
  }

  scoped_refptr<I420BufferInterface> i420 =
      frame.video_frame_buffer()->ToI420();
  if (!i420 || i420->width() != width_ || i420->height() != height_) {
    RTC_LOG(LS_ERROR) << "AMF: frame isn't the " << width_ << "x" << height_
                      << " I420 the encoder was started for.";
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }

  amf::AMFSurfacePtr surface;
  if (context_->AllocSurface(amf::AMF_MEMORY_HOST, amf::AMF_SURFACE_NV12,
                             width_, height_, &surface) != AMF_OK ||
      !surface) {
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }
  amf::AMFPlane* y = surface->GetPlane(amf::AMF_PLANE_Y);
  amf::AMFPlane* uv = surface->GetPlane(amf::AMF_PLANE_UV);
  if (!y || !uv || !y->GetNative() || !uv->GetNative() ||
      y->GetHPitch() < width_ || uv->GetHPitch() < width_) {
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }
  libyuv::I420ToNV12(i420->DataY(), i420->StrideY(), i420->DataU(),
                     i420->StrideU(), i420->DataV(), i420->StrideV(),
                     static_cast<uint8_t*>(y->GetNative()), y->GetHPitch(),
                     static_cast<uint8_t*>(uv->GetNative()), uv->GetHPitch(),
                     width_, height_);

  const int64_t pts = next_pts_++;
  surface->SetPts(pts);
  if (key_frame) {
    if (codec_ == AmfCodec::kAv1) {
      Set(surface, AMF_VIDEO_ENCODER_AV1_FORCE_FRAME_TYPE,
          amf_int64{AMF_VIDEO_ENCODER_AV1_FORCE_FRAME_TYPE_KEY});
      Set(surface, AMF_VIDEO_ENCODER_AV1_FORCE_INSERT_SEQUENCE_HEADER, true);
    } else {
      Set(surface, AMF_VIDEO_ENCODER_FORCE_PICTURE_TYPE,
          amf_int64{AMF_VIDEO_ENCODER_PICTURE_TYPE_IDR});
      Set(surface, AMF_VIDEO_ENCODER_INSERT_SPS, true);
      Set(surface, AMF_VIDEO_ENCODER_INSERT_PPS, true);
    }
  }

  AMF_RESULT res = encoder_->SubmitInput(surface);
  if (res == AMF_INPUT_FULL && !pending_.empty()) {
    Drain(pending_.front().pts, kEncodeWaitMs);
    res = encoder_->SubmitInput(surface);
  }
  if (res == AMF_INPUT_FULL) {
    // Dropped. The next frame still predicts from the last one encoded.
    RTC_LOG(LS_WARNING) << "AMF: encoder is backed up; dropping a frame.";
    return WEBRTC_VIDEO_CODEC_OK;
  }
  if (res != AMF_OK && res != AMF_NEED_MORE_INPUT) {
    RTC_LOG(LS_ERROR) << "AMF: SubmitInput failed (" << res << ").";
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }
  if (key_frame) {
    key_frame_request_ = false;
  }

  if (pending_.size() >= kMaxPending) {
    pending_.pop_front();
  }
  pending_.push_back({pts, frame.rtp_timestamp(), frame.ntp_time_ms(),
                      frame.render_time_ms(), frame.rotation(),
                      frame.color_space()});
  return Drain(pts, kEncodeWaitMs);
}

int32_t AmfVideoEncoder::Drain(int64_t until_pts, int wait_ms) {
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::milliseconds(wait_ms);
  for (;;) {
    amf::AMFDataPtr data;
    const AMF_RESULT res = encoder_->QueryOutput(&data);
    if (data) {
      amf::AMFBufferPtr buffer(data);
      if (!buffer || pending_.empty()) {
        continue;
      }
      // Output keeps input order, so frames queued before this one made
      // nothing. A pts we never set means the driver didn't carry it over:
      // take the oldest frame.
      const int64_t pts = data->GetPts();
      auto match =
          std::find_if(pending_.begin(), pending_.end(),
                       [pts](const Pending& p) { return p.pts == pts; });
      if (match == pending_.end()) {
        match = pending_.begin();
      }
      const Pending frame = *match;
      pending_.erase(pending_.begin(), match + 1);
      Deliver(buffer, frame);
      if (frame.pts == until_pts) {
        return WEBRTC_VIDEO_CODEC_OK;
      }
      continue;
    }
    if (res != AMF_OK && res != AMF_REPEAT && res != AMF_NEED_MORE_INPUT) {
      RTC_LOG(LS_ERROR) << "AMF: QueryOutput failed (" << res << ").";
      return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
    }
    if (std::chrono::steady_clock::now() >= deadline) {
      return WEBRTC_VIDEO_CODEC_OK;
    }
    // Only reached quickly on drivers without a query timeout.
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
}

void AmfVideoEncoder::Deliver(amf::AMFBuffer* buffer, const Pending& frame) {
  const uint8_t* data = static_cast<const uint8_t*>(buffer->GetNative());
  const size_t size = buffer->GetSize();
  if (!data || size == 0) {
    return;
  }
  std::vector<uint8_t> packet(data, data + size);

  bool key_frame = false;
  int qp = -1;
  CodecSpecificInfo info;
  if (codec_ == AmfCodec::kAv1) {
    amf_int64 type = -1;
    buffer->GetProperty(AMF_VIDEO_ENCODER_AV1_OUTPUT_FRAME_TYPE, &type);
    key_frame = type == AMF_VIDEO_ENCODER_AV1_OUTPUT_FRAME_TYPE_KEY;

    livekit::av1::NormalizeForRtp(&packet);
    if (key_frame) {
      livekit::av1::EnsureSequenceHeaderOnKeyframe(&packet, headers_);
    }
    if (packet.empty() ||
        !livekit::av1::IsWebRtcParseable(packet.data(), packet.size())) {
      // The frames after this one predict from it, so start over.
      RTC_LOG(LS_ERROR) << "AMF: AV1 output isn't packetizable; dropping it.";
      key_frame_request_ = true;
      return;
    }

    info.codecType = kVideoCodecAV1;
    info.end_of_picture = true;
    info.scalability_mode = ScalabilityMode::kL1T1;
    const auto layers = svc_controller_.NextFrameConfig(/*restart=*/key_frame);
    if (!layers.empty()) {
      info.generic_frame_info = svc_controller_.OnEncodeDone(layers.front());
      if (layers.front().IsKeyframe()) {
        info.template_structure = svc_controller_.DependencyStructure();
      }
    }
  } else {
    bool has_sps = false;
    for (const H264::NaluIndex& nalu : H264::FindNaluIndices(packet)) {
      if (nalu.payload_size == 0) {
        continue;
      }
      const H264::NaluType type =
          H264::ParseNaluType(packet[nalu.payload_start_offset]);
      key_frame |= type == H264::kIdr;
      has_sps |= type == H264::kSps;
    }
    if (key_frame && !has_sps) {
      packet.insert(packet.begin(), headers_.begin(), headers_.end());
    }
    info.codecType = kVideoCodecH264;
    info.codecSpecific.H264.packetization_mode =
        H264PacketizationMode::NonInterleaved;
  }

  EncodedImage image;
  image.SetEncodedData(
      EncodedImageBuffer::Create(packet.data(), packet.size()));
  image._encodedWidth = width_;
  image._encodedHeight = height_;
  image.SetRtpTimestamp(frame.rtp_timestamp);
  image.SetSimulcastIndex(0);
  image.ntp_time_ms_ = frame.ntp_time_ms;
  image.capture_time_ms_ = frame.capture_time_ms;
  image.rotation_ = frame.rotation;
  image.content_type_ = VideoContentType::UNSPECIFIED;
  image.timing_.flags = VideoSendTiming::kInvalid;
  image._frameType = key_frame ? VideoFrameType::kVideoFrameKey
                               : VideoFrameType::kVideoFrameDelta;
  image.SetColorSpace(frame.color_space);
  if (codec_ == AmfCodec::kH264) {
    h264_parser_.ParseBitstream(image);
    qp = h264_parser_.GetLastSliceQp().value_or(-1);
  }
  image.qp_ = qp;

  // A failed send (a layer paused by dynacast, say) isn't an encode error.
  const auto result = callback_->OnEncodedImage(image, &info);
  if (result.error != EncodedImageCallback::Result::OK) {
    RTC_LOG(LS_WARNING) << "AMF: encoded frame wasn't taken (" << result.error
                        << ").";
  }
}

void AmfVideoEncoder::SetRates(const RateControlParameters& parameters) {
  if (!encoder_) {
    return;
  }
  if (!std::isfinite(parameters.framerate_fps) ||
      parameters.framerate_fps < 1.0) {
    return;
  }
  const uint32_t bitrate_bps = parameters.bitrate.get_sum_bps();
  if (bitrate_bps == 0) {
    sending_ = false;
    return;
  }
  if (!sending_) {
    // Whoever watches next has nothing to predict from.
    sending_ = true;
    key_frame_request_ = true;
  }
  bitrate_bps_ = bitrate_bps;
  const uint32_t fps = static_cast<uint32_t>(
      std::clamp(std::round(parameters.framerate_fps), 1.0, 240.0));
  // WebRTC reports the measured rate, which wanders by a frame or two. Only
  // a real change is worth reconfiguring the encoder's rate control for.
  const bool fps_changed =
      std::abs(static_cast<int>(fps) - static_cast<int>(fps_)) * 5 >=
      static_cast<int>(fps_);
  if (fps_changed) {
    fps_ = fps;
  }
  ApplyRates(fps_changed);
}

void AmfVideoEncoder::ApplyRates(bool with_fps) {
  const amf_int64 bitrate = bitrate_bps_;
  if (codec_ == AmfCodec::kAv1) {
    Set(encoder_, AMF_VIDEO_ENCODER_AV1_TARGET_BITRATE, bitrate);
    Set(encoder_, AMF_VIDEO_ENCODER_AV1_PEAK_BITRATE, bitrate);
    Set(encoder_, AMF_VIDEO_ENCODER_AV1_VBV_BUFFER_SIZE, bitrate);
    if (with_fps) {
      Set(encoder_, AMF_VIDEO_ENCODER_AV1_FRAMERATE,
          AMFConstructRate(fps_, 1));
    }
  } else {
    Set(encoder_, AMF_VIDEO_ENCODER_TARGET_BITRATE, bitrate);
    Set(encoder_, AMF_VIDEO_ENCODER_PEAK_BITRATE, bitrate);
    Set(encoder_, AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE, bitrate);
    if (with_fps) {
      Set(encoder_, AMF_VIDEO_ENCODER_FRAMERATE, AMFConstructRate(fps_, 1));
    }
  }
}

VideoEncoder::EncoderInfo AmfVideoEncoder::GetEncoderInfo() const {
  EncoderInfo info;
  info.supports_native_handle = false;
  info.implementation_name =
      codec_ == AmfCodec::kAv1 ? "AMD AMF AV1 Encoder" : "AMD AMF H264 Encoder";
  info.scaling_settings = ScalingSettings::kOff;
  info.is_hardware_accelerated = true;
  info.supports_simulcast = false;
  info.requested_resolution_alignment = 2;
  info.preferred_pixel_formats = {VideoFrameBuffer::Type::kI420};
  return info;
}

}  // namespace webrtc
