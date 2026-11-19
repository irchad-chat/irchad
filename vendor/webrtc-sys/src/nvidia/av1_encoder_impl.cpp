#include "av1_encoder_impl.h"

#include <algorithm>
#include <cmath>
#include <limits>
#include <utility>

#include "../av1_bitstream.h"
#include "i420_buffer_cuda.h"
#include "api/video/video_codec_constants.h"
#include "common_video/libyuv/include/webrtc_libyuv.h"
#include "modules/video_coding/include/video_codec_interface.h"
#include "modules/video_coding/include/video_error_codes.h"
#include "modules/video_coding/svc/create_scalability_structure.h"
#include "modules/video_coding/utility/simulcast_rate_allocator.h"
#include "rtc_base/checks.h"
#include "rtc_base/logging.h"
#include "system_wrappers/include/metrics.h"

namespace webrtc {

// Used by histograms. Values of entries should not be changed.
enum AV1EncoderImplEvent {
  kAV1EncoderEventInit = 0,
  kAV1EncoderEventError = 1,
  kAV1EncoderEventMax = 16,
};

namespace {

uint32_t ClampToUint32(uint64_t value) {
  return static_cast<uint32_t>(
      std::min<uint64_t>(value, std::numeric_limits<uint32_t>::max()));
}

}  // namespace

NvidiaAV1EncoderImpl::NvidiaAV1EncoderImpl(
    const webrtc::Environment& env,
    CUcontext context,
    CUmemorytype memory_type,
    NV_ENC_BUFFER_FORMAT nv_format,
    const SdpVideoFormat& format)
    : env_(env),
      encoder_(nullptr),
      cu_context_(context),
      cu_memory_type_(memory_type),
      cu_scaled_array_(nullptr),
      nv_format_(nv_format),
      format_(format) {
  RTC_CHECK_NE(cu_memory_type_, CU_MEMORYTYPE_HOST);
}

NvidiaAV1EncoderImpl::~NvidiaAV1EncoderImpl() {
  Release();
}

void NvidiaAV1EncoderImpl::ReportInit() {
  if (has_reported_init_) {
    return;
  }
  RTC_HISTOGRAM_ENUMERATION("WebRTC.Video.AV1EncoderImpl.Event",
                            kAV1EncoderEventInit, kAV1EncoderEventMax);
  has_reported_init_ = true;
}

void NvidiaAV1EncoderImpl::ReportError() {
  if (has_reported_error_) {
    return;
  }
  RTC_HISTOGRAM_ENUMERATION("WebRTC.Video.AV1EncoderImpl.Event",
                            kAV1EncoderEventError, kAV1EncoderEventMax);
  has_reported_error_ = true;
}

int32_t NvidiaAV1EncoderImpl::InitEncode(
    const VideoCodec* inst,
    const VideoEncoder::Settings& settings) {
  (void)settings;
  if (!inst || inst->codecType != kVideoCodecAV1) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  if (inst->maxFramerate == 0) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  if (inst->width < 1 || inst->height < 1) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  if (inst->numberOfSimulcastStreams > 1) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  const auto scalability_mode = inst->GetScalabilityMode();
  if (scalability_mode.has_value() &&
      *scalability_mode != ScalabilityMode::kL1T1 &&
      *scalability_mode != ScalabilityMode::kL1T2) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }

  int32_t release_ret = Release();
  if (release_ret != WEBRTC_VIDEO_CODEC_OK) {
    ReportError();
    return release_ret;
  }

  codec_ = *inst;
  sent_decodable_keyframe_ = false;
  cached_sequence_header_obu_.clear();
  if (!codec_.GetScalabilityMode().has_value()) {
    codec_.SetScalabilityMode(ScalabilityMode::kL1T1);
  }
  // irchad: L1T2 makes every other frame one nothing references. A viewer
  // that loses a packet of one skips that frame instead of freezing until a
  // keyframe, and the SFU can forward the base layer alone (half the frame
  // rate) to a viewer it can't keep up with. The layers live only in the
  // dependency descriptor: the bitstream is plain single-layer AV1 with some
  // non-reference frames (no temporal ids, one operating point), so every
  // decoder takes it as is.
  svc_controller_ = CreateScalabilityStructure(*codec_.GetScalabilityMode());
  if (!svc_controller_) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  layered_ = *codec_.GetScalabilityMode() != ScalabilityMode::kL1T1;
  frames_since_key_ = 0;

  if (codec_.numberOfSimulcastStreams == 0) {
    codec_.simulcastStream[0].width = codec_.width;
    codec_.simulcastStream[0].height = codec_.height;
  }

  const size_t new_capacity =
      CalcBufferSize(VideoType::kI420, codec_.width, codec_.height);
  encoded_image_.SetEncodedData(EncodedImageBuffer::Create(new_capacity));
  encoded_image_._encodedWidth = codec_.width;
  encoded_image_._encodedHeight = codec_.height;
  encoded_image_.set_size(0);

  configuration_.sending = false;
  configuration_.frame_dropping_on = codec_.GetFrameDropEnabled();
  configuration_.key_frame_interval =
      std::max<int>(1, static_cast<int>(codec_.maxFramerate)) * 2;

  configuration_.width = codec_.width;
  configuration_.height = codec_.height;

  configuration_.max_frame_rate = codec_.maxFramerate;
  configuration_.target_bps = codec_.startBitrate * 1000;
  configuration_.max_bps = codec_.maxBitrate * 1000;

  const CUresult result = cuCtxSetCurrent(cu_context_);
  if (result != CUDA_SUCCESS) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }

  try {
    if (cu_memory_type_ == CU_MEMORYTYPE_DEVICE) {
      encoder_ = std::make_unique<NvEncoderCuda>(cu_context_, codec_.width,
                                                 codec_.height, nv_format_, 0);
    } else {
      RTC_DCHECK_NOTREACHED();
    }
  } catch (const NVENCException& e) {
    RTC_LOG(LS_ERROR) << "Failed Initialize NvEncoder " << e.what();
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERROR;
  }

  nv_initialize_params_.version = NV_ENC_INITIALIZE_PARAMS_VER;
  nv_encode_config_.version = NV_ENC_CONFIG_VER;
  nv_initialize_params_.encodeConfig = &nv_encode_config_;

  GUID encodeGuid = NV_ENC_CODEC_AV1_GUID;
  GUID presetGuid = NV_ENC_PRESET_P4_GUID;

  encoder_->CreateDefaultEncoderParams(&nv_initialize_params_, encodeGuid,
                                       presetGuid,
                                       NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY);

  nv_initialize_params_.frameRateNum = std::max<uint32_t>(
      1, static_cast<uint32_t>(std::round(configuration_.max_frame_rate)));
  nv_initialize_params_.frameRateDen = 1;
  nv_initialize_params_.bufferFormat = nv_format_;
  // irchad: refPicFlag, which makes a frame non-reference, only applies when
  // the client decides picture types.
  if (layered_) {
    nv_initialize_params_.enablePTD = 0;
  }

  nv_encode_config_.profileGUID = NV_ENC_AV1_PROFILE_MAIN_GUID;
  nv_encode_config_.gopLength = NVENC_INFINITE_GOPLENGTH;
  nv_encode_config_.frameIntervalP = 1;
  nv_encode_config_.encodeCodecConfig.av1Config.level =
      NV_ENC_LEVEL_AV1_AUTOSELECT;
  nv_encode_config_.encodeCodecConfig.av1Config.tier = NV_ENC_TIER_AV1_0;
  nv_encode_config_.encodeCodecConfig.av1Config.chromaFormatIDC = 1;
  nv_encode_config_.encodeCodecConfig.av1Config.inputPixelBitDepthMinus8 = 0;
  nv_encode_config_.encodeCodecConfig.av1Config.pixelBitDepthMinus8 = 0;
  nv_encode_config_.encodeCodecConfig.av1Config.idrPeriod =
      NVENC_INFINITE_GOPLENGTH;
  nv_encode_config_.encodeCodecConfig.av1Config.repeatSeqHdr = 1;
  nv_encode_config_.encodeCodecConfig.av1Config.maxTemporalLayersMinus1 = 0;
  nv_encode_config_.rcParams.version = NV_ENC_RC_PARAMS_VER;
  nv_encode_config_.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
  nv_encode_config_.rcParams.averageBitRate = configuration_.target_bps;
  const uint64_t vbv_buffer_size =
      (static_cast<uint64_t>(nv_encode_config_.rcParams.averageBitRate) *
       nv_initialize_params_.frameRateDen /
       nv_initialize_params_.frameRateNum) *
      5;
  nv_encode_config_.rcParams.vbvBufferSize =
      ClampToUint32(vbv_buffer_size);
  nv_encode_config_.rcParams.vbvInitialDelay =
      nv_encode_config_.rcParams.vbvBufferSize;

  try {
    encoder_->CreateEncoder(&nv_initialize_params_);
  } catch (const NVENCException& e) {
    RTC_LOG(LS_ERROR) << "Failed Initialize NvEncoder " << e.what();
    ReportError();
    return WEBRTC_VIDEO_CODEC_ERROR;
  }

  RTC_LOG(LS_INFO) << "NVIDIA AV1 NVENC initialized: " << codec_.width << "x"
                   << codec_.height << " @ " << codec_.maxFramerate
                   << "fps, target_bps=" << configuration_.target_bps;

  ReportInit();

  SimulcastRateAllocator init_allocator(env_, codec_);
  VideoBitrateAllocation allocation =
      init_allocator.Allocate(VideoBitrateAllocationParameters(
          DataRate::KilobitsPerSec(codec_.startBitrate), codec_.maxFramerate));
  SetRates(RateControlParameters(allocation, codec_.maxFramerate));
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAV1EncoderImpl::RegisterEncodeCompleteCallback(
    EncodedImageCallback* callback) {
  encoded_image_callback_ = callback;
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAV1EncoderImpl::Release() {
  if (encoder_) {
    encoder_->DestroyEncoder();
    encoder_ = nullptr;
  }
  if (cu_scaled_array_) {
    cuArrayDestroy(cu_scaled_array_);
    cu_scaled_array_ = nullptr;
  }
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAV1EncoderImpl::Encode(
    const VideoFrame& input_frame,
    const std::vector<VideoFrameType>* frame_types) {
  if (!encoder_) {
    ReportError();
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }
  if (!encoded_image_callback_) {
    RTC_LOG(LS_WARNING)
        << "InitEncode() has been called, but a callback function "
           "has not been set with RegisterEncodeCompleteCallback()";
    ReportError();
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }

  webrtc::scoped_refptr<I420BufferInterface> frame_buffer =
      input_frame.video_frame_buffer()->ToI420();
  if (!frame_buffer) {
    RTC_LOG(LS_ERROR) << "Failed to convert "
                      << VideoFrameBufferTypeToString(
                             input_frame.video_frame_buffer()->type())
                      << " image to I420. Can't encode frame.";
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }
  RTC_CHECK(frame_buffer->type() == VideoFrameBuffer::Type::kI420);

  bool is_keyframe_needed = false;
  if (configuration_.key_frame_request && configuration_.sending) {
    is_keyframe_needed = true;
  }
  if (frame_types && !frame_types->empty()) {
    if ((*frame_types)[0] == VideoFrameType::kVideoFrameKey) {
      is_keyframe_needed = true;
    }
    if ((*frame_types)[0] == VideoFrameType::kEmptyFrame) {
      return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
    }
  }

  RTC_DCHECK_EQ(configuration_.width, frame_buffer->width());
  RTC_DCHECK_EQ(configuration_.height, frame_buffer->height());

  if (!configuration_.sending) {
    return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
  }

  if (!sent_decodable_keyframe_) {
    is_keyframe_needed = true;
  }

  // irchad: the layer this frame belongs to, decided before it's encoded, as
  // it says whether NVENC may keep the frame as a reference.
  std::vector<ScalableVideoController::LayerFrameConfig> layer_frames =
      svc_controller_->NextFrameConfig(/*restart=*/is_keyframe_needed);
  if (layer_frames.empty()) {
    return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
  }
  const ScalableVideoController::LayerFrameConfig layer_frame =
      layer_frames.front();
  if (layer_frame.IsKeyframe()) {
    is_keyframe_needed = true;
  }
  const bool is_reference =
      std::any_of(layer_frame.Buffers().begin(), layer_frame.Buffers().end(),
                  [](const CodecBufferUsage& b) { return b.updated; });
  // Each frame the controller hands out must reach the callback, or the
  // dependency descriptor's frame diffs no longer match what was sent: on
  // any miss, start over from a keyframe.
  bool delivered = false;

  try {
    const NvEncInputFrame* nv_enc_input_frame = encoder_->GetNextInputFrame();

    if (cu_memory_type_ == CU_MEMORYTYPE_DEVICE) {
      CopyI420BufferToDeviceFrame(cu_context_, *frame_buffer,
                                  *nv_enc_input_frame);
    }

    NV_ENC_PIC_PARAMS pic_params = NV_ENC_PIC_PARAMS();
    pic_params.version = NV_ENC_PIC_PARAMS_VER;
    pic_params.encodePicFlags = 0;
    if (is_keyframe_needed) {
      pic_params.encodePicFlags = NV_ENC_PIC_FLAG_FORCEINTRA |
                                  NV_ENC_PIC_FLAG_FORCEIDR |
                                  NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
    }
    if (layered_) {
      if (is_keyframe_needed) {
        frames_since_key_ = 0;
      }
      pic_params.pictureType =
          is_keyframe_needed ? NV_ENC_PIC_TYPE_IDR : NV_ENC_PIC_TYPE_P;
      NV_ENC_PIC_PARAMS_AV1& av1 = pic_params.codecPicParams.av1PicParams;
      av1.displayPOCSyntax = frames_since_key_++;
      av1.refPicFlag = (is_keyframe_needed || is_reference) ? 1 : 0;
      // Left at 0 on purpose: a temporal id would put layer markers in the
      // bitstream, which the decoders would then have to handle.
      av1.temporalId = 0;
    }

    std::vector<std::vector<uint8_t>> bit_stream;
    encoder_->EncodeFrame(bit_stream, &pic_params);

    for (std::vector<uint8_t>& packet : bit_stream) {
      if (delivered) {
        RTC_LOG(LS_WARNING)
            << "NVIDIA AV1 NVENC returned a second packet for one frame; "
               "dropping it and restarting from a keyframe.";
        configuration_.key_frame_request = true;
        break;
      }
      if (packet.empty()) {
        RTC_LOG(LS_WARNING)
            << "NVIDIA AV1 NVENC returned empty packet; skipping output.";
        continue;
      }

      livekit::av1::NormalizeForRtp(&packet);
      if (packet.empty()) {
        RTC_LOG(LS_ERROR)
            << "NVIDIA AV1 NVENC packet contained no transferable OBUs "
               "after RTP normalization; skipping.";
        continue;
      }

      std::vector<uint8_t> sequence_header;
      if (livekit::av1::ExtractSequenceHeaderObu(
              packet.data(), packet.size(), &sequence_header)) {
        cached_sequence_header_obu_ = std::move(sequence_header);
      }

      const bool treat_as_keyframe = is_keyframe_needed;
      if (treat_as_keyframe) {
        livekit::av1::EnsureSequenceHeaderOnKeyframe(
            &packet, cached_sequence_header_obu_);
      }

      if (!livekit::av1::IsWebRtcParseable(packet.data(), packet.size())) {
        RTC_LOG(LS_ERROR)
            << "NVIDIA AV1 NVENC bitstream is not parseable by WebRTC; "
               "dropping frame (size="
            << packet.size() << ").";
        continue;
      }

      if (treat_as_keyframe &&
          livekit::av1::HasSequenceHeaderObu(packet.data(), packet.size())) {
        sent_decodable_keyframe_ = true;
        configuration_.key_frame_request = false;
      } else if (!sent_decodable_keyframe_) {
        RTC_LOG(LS_WARNING)
            << "NVIDIA AV1 NVENC keyframe missing sequence header OBU; "
               "continuing to force keyframes.";
        configuration_.key_frame_request = true;
      } else if (is_keyframe_needed) {
        configuration_.key_frame_request = false;
      }

      int32_t result = ProcessEncodedFrame(packet, input_frame,
                                           treat_as_keyframe, layer_frame);
      if (result != WEBRTC_VIDEO_CODEC_OK) {
        configuration_.key_frame_request = true;
        return result;
      }
      delivered = true;
    }
  } catch (const NVENCException& e) {
    RTC_LOG(LS_ERROR) << "Failed EncodeFrame NvEncoder " << e.what();
    configuration_.key_frame_request = true;
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }

  if (!delivered) {
    configuration_.key_frame_request = true;
    return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
  }
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAV1EncoderImpl::ProcessEncodedFrame(
    const std::vector<uint8_t>& packet,
    const ::webrtc::VideoFrame& input_frame,
    bool is_keyframe,
    const ScalableVideoController::LayerFrameConfig& layer_frame) {
  encoded_image_._encodedWidth = encoder_->GetEncodeWidth();
  encoded_image_._encodedHeight = encoder_->GetEncodeHeight();
  encoded_image_.SetRtpTimestamp(input_frame.rtp_timestamp());
  encoded_image_.SetSimulcastIndex(0);
  encoded_image_.ntp_time_ms_ = input_frame.ntp_time_ms();
  encoded_image_.capture_time_ms_ = input_frame.render_time_ms();
  encoded_image_.rotation_ = input_frame.rotation();
  encoded_image_.content_type_ = VideoContentType::UNSPECIFIED;
  encoded_image_.timing_.flags = VideoSendTiming::kInvalid;
  encoded_image_._frameType =
      is_keyframe ? VideoFrameType::kVideoFrameKey
                  : VideoFrameType::kVideoFrameDelta;
  encoded_image_.SetColorSpace(input_frame.color_space());

  encoded_image_.SetEncodedData(
      EncodedImageBuffer::Create(packet.data(), packet.size()));
  encoded_image_.set_size(packet.size());

  encoded_image_.qp_ = -1;

  CodecSpecificInfo codecInfo;
  codecInfo.codecType = kVideoCodecAV1;
  codecInfo.end_of_picture = true;
  codecInfo.scalability_mode = codec_.GetScalabilityMode();

  codecInfo.generic_frame_info = svc_controller_->OnEncodeDone(layer_frame);
  if (layer_frame.IsKeyframe()) {
    codecInfo.template_structure = svc_controller_->DependencyStructure();
  }

  const auto result =
      encoded_image_callback_->OnEncodedImage(encoded_image_, &codecInfo);
  // The frame has already been delivered, so a failed send (for example, a
  // simulcast layer paused by dynacast) must not be reported as an encode
  // error: SimulcastEncoderAdapter would drop an RTP timestamp that its
  // callback has already consumed.
  if (result.error != EncodedImageCallback::Result::OK) {
    RTC_LOG(LS_WARNING) << "Encode m_encodedCompleteCallback failed "
                        << result.error;
  }
  return WEBRTC_VIDEO_CODEC_OK;
}

VideoEncoder::EncoderInfo NvidiaAV1EncoderImpl::GetEncoderInfo() const {
  EncoderInfo info;
  info.supports_native_handle = false;
  info.implementation_name = "NVIDIA AV1 Encoder";
  info.scaling_settings = VideoEncoder::ScalingSettings::kOff;
  info.is_hardware_accelerated = true;
  info.supports_simulcast = false;
  info.preferred_pixel_formats = {VideoFrameBuffer::Type::kI420};
  return info;
}

void NvidiaAV1EncoderImpl::SetRates(
    const RateControlParameters& parameters) {
  if (!encoder_) {
    RTC_LOG(LS_WARNING) << "SetRates() while uninitialized.";
    return;
  }

  const uint32_t target_bps = parameters.bitrate.get_sum_bps();
  if (target_bps == 0) {
    configuration_.SetStreamState(false);
    return;
  }

  // irchad: NVENC's CBR gives each frame bitrate / frame rate, by frame count
  // and not by time. libwebrtc's framerate_fps is the input rate over the last
  // second, so after a capture stall it's low and the frames that follow get
  // several times their share: the bitrate overshoots and the pacer queues
  // for seconds. The configured rate errs the other way, to softer frames
  // while capture really is slow. A new max rate comes through InitEncode.
  const uint32_t frame_rate = std::max<uint32_t>(1, codec_.maxFramerate);
  try {
    if (!encoder_->SetRates(frame_rate, target_bps)) {
      RTC_LOG(LS_WARNING) << "Failed to reconfigure NVENC rates: encoder is "
                             "not initialized.";
      return;
    }
  } catch (const NVENCException& e) {
    RTC_LOG(LS_WARNING) << "Failed to reconfigure NVENC rates to "
                        << target_bps << " bps @ " << frame_rate
                        << " fps: " << e.what();
    return;
  }

  codec_.maxBitrate = target_bps / 1000;
  configuration_.target_bps = target_bps;
  configuration_.SetStreamState(true);
}

void NvidiaAV1EncoderImpl::LayerConfig::SetStreamState(bool send_stream) {
  if (send_stream && !sending) {
    key_frame_request = true;
  }
  sending = send_stream;
}

}  // namespace webrtc
