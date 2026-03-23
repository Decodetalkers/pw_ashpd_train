use std::os::fd::{AsRawFd, OwnedFd};

use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream,
};
use gstreamer as gst;
use gstreamer::prelude::*;
use signal_hook::{consts::SIGINT, iterator::Signals};

async fn open_portal() -> ashpd::Result<(Stream, OwnedFd)> {
    let proxy = Screencast::new().await?;
    let session = proxy.create_session(Default::default()).await?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window | SourceType::Virtual)
                .set_multiple(false)
                .set_restore_token(None)
                .set_persist_mode(ashpd::desktop::PersistMode::ExplicitlyRevoked),
        )
        .await?;

    let response = proxy
        .start(&session, None, Default::default())
        .await?
        .response()?;
    let stream = response
        .streams()
        .first()
        .expect("No stream found or selected")
        .to_owned();

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await?;

    Ok((stream, fd))
}

#[tokio::main]
async fn main() -> ashpd::Result<()> {
    gst::init().unwrap();

    let (stream, stream_fd) = open_portal().await?;
    let pipewire_node_id = &stream.pipe_wire_node_id();
    let stream_raw_fd = &stream_fd.as_raw_fd();

    let pipewire_source = gst::ElementFactory::make("pipewiresrc")
        .name("pw")
        .property("fd", stream_raw_fd)
        .property("path", pipewire_node_id.to_string())
        .build()
        .unwrap();

    let videoconvert = gst::ElementFactory::make("videoconvert")
        .name("videoconvert1")
        .build()
        .unwrap();
    let videoconvert2 = gst::ElementFactory::make("videoconvert")
        .name("videoconvert2")
        .build()
        .unwrap();
    let x264enc = gst::ElementFactory::make("x264enc")
        .property_from_str("tune", "zerolatency")
        .build()
        .unwrap();
    let flvmux = gst::ElementFactory::make("flvmux").build().unwrap();
    let filesink = gst::ElementFactory::make("filesink")
        .property("location", "xyz.flv")
        .build()
        .unwrap();
    let wayland_sink = gst::ElementFactory::make("waylandsink").build().unwrap();
    let queue1 = gst::ElementFactory::make("queue")
        .name("queue1")
        .property("max-size-buffers", 50_u32)
        .property("max-size-bytes", 0_u32)
        .property("max-size-time", 0_u64)
        .build()
        .unwrap();
    let queue2 = gst::ElementFactory::make("queue")
        .name("queue2")
        .property("max-size-buffers", 50_u32)
        .property("max-size-bytes", 0_u32)
        .property("max-size-time", 0_u64)
        .build()
        .unwrap();
    let tee = gst::ElementFactory::make("tee")
        .name("tee")
        .build()
        .unwrap();
    let pipeline = gst::Pipeline::default();
    pipeline
        .add_many([
            &pipewire_source,
            &tee,
            &videoconvert,
            &videoconvert2,
            &x264enc,
            &flvmux,
            &filesink,
            &wayland_sink,
            &queue1,
            &queue2,
        ])
        .unwrap();
    gst::Element::link_many([&pipewire_source, &tee]).unwrap();
    gst::Element::link_many([&tee, &queue1, &videoconvert, &x264enc, &flvmux, &filesink]).unwrap();
    gst::Element::link_many([&tee, &queue2, &videoconvert2, &wayland_sink]).unwrap();

    //let tee_pad_1 = tee.request_pad_simple("src_%u").unwrap();
    //let queue1_pad = queue1.static_pad("sink").unwrap();
    //let tee_pad_2 = tee.request_pad_simple("src_%u").unwrap();
    //let queue2_pad = queue2.static_pad("sink").unwrap();
    //tee_pad_1.link(&queue1_pad).unwrap();
    //tee_pad_2.link(&queue2_pad).unwrap();
    //drop(queue2_pad);
    //drop(queue1_pad);

    let pipeline_2 = pipeline.clone();
    let mut signals = Signals::new([SIGINT]).unwrap();
    let handle = std::thread::spawn(move || {
        for sig in signals.forever() {
            if sig == SIGINT {
                pipeline_2.send_event(gst::event::Eos::new());

                return;
            }
        }
    });

    pipeline.set_state(gst::State::Playing).unwrap();
    let bus = pipeline.bus().unwrap();

    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        use gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => {
                pipeline.set_state(gst::State::Null).unwrap();
                println!("EOS");
                break;
            }
            MessageView::Error(err) => {
                pipeline.set_state(gst::State::Null).unwrap();
                eprintln!(
                    "Got error from {}: {} ({})",
                    msg.src()
                        .map(|s| String::from(s.path_string()))
                        .unwrap_or_else(|| "None".into()),
                    err.error(),
                    err.debug().unwrap_or_else(|| "".into()),
                );
                break;
            }

            _ => (),
        }
    }
    handle.join().unwrap();

    Ok(())
}
