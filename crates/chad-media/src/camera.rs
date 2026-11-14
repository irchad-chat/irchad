//! Our camera in the call: a platform capture feeds the screen share's
//! encoder at 720p 30 fps, published as LiveKit's camera source (simulcast,
//! the SDK's default codec). Watching one is watching a share (`room::watch`).

use livekit::options::TrackPublishOptions;
use livekit::track::{LocalTrack, LocalVideoTrack, TrackSource};
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution, native::NativeVideoSource};

use crate::Voice;
use crate::engine::{PREVIEW_CAMERA, lock};
use crate::screen::{OnEnd, Preset, Share, encoder};

/// `livekit.TrackSource.CAMERA` in the protocol; the SDK doesn't export the enum.
const PROTO_CAMERA: i32 = 1;

pub(crate) async fn start(voice: &Voice) -> Result<(), String> {
    let shared = &voice.shared;
    let room = lock(&shared.room)
        .clone()
        .ok_or("Join voice to turn on your camera.")?;
    // LiveKit doesn't answer a publish it won't allow; it just times out.
    let allowed = room.local_participant().permission().is_none_or(|p| {
        p.can_publish
            && (p.can_publish_sources.is_empty() || p.can_publish_sources.contains(&PROTO_CAMERA))
    });
    if !allowed {
        return Err("This server doesn't allow cameras. Rejoin voice after it's updated.".into());
    }
    voice.stop_camera();
    let id = shared.next_share();
    let source = NativeVideoSource::new(
        VideoResolution {
            width: 1280,
            height: 720,
        },
        false,
    );
    let sink = encoder(
        source.clone(),
        Preset::P720p30,
        shared.clone(),
        PREVIEW_CAMERA,
    );
    let v = voice.clone();
    let on_end: OnEnd = Box::new(move || v.end_share(id, true));
    let capture = crate::platform::camera(shared.settings().camera, sink, on_end).await?;
    let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source));
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track),
            TrackPublishOptions {
                source: TrackSource::Camera,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| {
            tracing::warn!("publishing the camera: {e}");
            "Couldn't turn on your camera.".to_owned()
        })?;
    shared.set_share(
        Share {
            id,
            _capture: capture,
            room,
            tracks: vec![publication.sid()],
            rt: tokio::runtime::Handle::current(),
            public: None,
        },
        true,
    );
    Ok(())
}
