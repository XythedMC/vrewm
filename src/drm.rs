use std::{collections::HashMap, path::Path};

use smithay::{
    backend::{
        allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        drm::{DrmDevice, DrmDeviceFd, compositor::FrameFlags, exporter::gbm::{GbmFramebufferExporter, NodeFilter}},
        egl::{context::EGLContext, display::EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{Color32F, ImportDma, element::surface::WaylandSurfaceRenderElement, gles::GlesRenderer},
        session::{Session, libseat::LibSeatSession, Event as SessionEvent},
        udev::{UdevBackend, UdevEvent},
    },
    desktop::space::SpaceRenderElements,
    output::{Mode as WlMode, Output, OutputModeSource, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, LoopHandle},
        drm::{buffer::DrmFourcc, control::{Device, connector::State, crtc::Handle}},
        input::Libinput,
    },
    utils::Size,
};
use rustix::fs::OFlags;
use crate::{Treewm, state::{GbmDrmCompositor, GpuData}};

fn open_gpu(
    device_id: u64,
    path: &Path,
    state: &mut Treewm,
    handle: &LoopHandle<'static, Treewm>,
) {
    eprintln!("opening GPU: {:?}", path);

    let fd = DrmDeviceFd::new(
        state.session.as_mut().unwrap()
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY)
            .expect("Failed to open GPU")
            .into(),
    );

    let (mut drm, drm_notifier) = DrmDevice::new(fd.clone(), true).expect("Invalid DRM node");
    let gbm = GbmDevice::new(fd.clone()).expect("Failed to init GBM");

    handle.insert_source(drm_notifier, move |event, _, state| {
        state.process_drm_event(device_id, event);
    }).unwrap();

    let egl_display = unsafe { EGLDisplay::new(gbm.clone()).expect("Failed to create EGL display") };
    let egl_ctx = EGLContext::new(&egl_display).expect("Failed to create EGL context");
    let mut renderer = unsafe { GlesRenderer::new(egl_ctx).expect("Failed to create GLES renderer") };

    let resources = drm.resource_handles().expect("Failed to get DRM resources");
    let mut compositors: HashMap<Handle, GbmDrmCompositor> = HashMap::new();

    for &connector_handle in resources.connectors() {
        let info = drm.get_connector(connector_handle, true).unwrap();
        if info.state() != State::Connected {
            continue;
        }

        let mode = info.modes()[0];
        let (mw, mh) = mode.size();

        let output = Output::new(
            format!("{:?}", connector_handle),
            PhysicalProperties {
                size: info.size()
                    .map(|(w, h)| (w as i32, h as i32).into())
                    .unwrap_or_default(),
                subpixel: Subpixel::Unknown,
                make: "Unknown".to_string(),
                model: "Unknown".to_string(),
                serial_number: "Unknown".to_string(),
            },
        );
        let wl_mode = WlMode {
            size: (mw as i32, mh as i32).into(),
            refresh: mode.vrefresh() as i32 * 1000,
        };
        output.change_current_state(Some(wl_mode), None, None, Some((0, 0).into()));
        output.set_preferred(wl_mode);
        output.create_global::<Treewm>(&state.display_handle);
        state.space.map_output(&output, (0, 0));

        for &encoder in info.encoders() {
            let encoder_info = drm.get_encoder(encoder).unwrap();
            let crtc = resources.filter_crtcs(encoder_info.possible_crtcs())[0];
            let surface = drm.create_surface(crtc, mode, &[connector_handle])
                .expect("Failed to create DRM surface");

            let Ok(mut compositor) = GbmDrmCompositor::new(
                OutputModeSource::from(&output),
                surface,
                None,
                GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT),
                GbmFramebufferExporter::new(gbm.clone(), NodeFilter::None),
                [DrmFourcc::Xrgb8888],
                renderer.dmabuf_formats(),
                Size::from((state.config.cursor_size[0], state.config.cursor_size[1])),
                Some(gbm.clone()),
            ) else {
                eprintln!("Skipping ctrc {:?}", crtc);
                continue;
            };

            compositor.render_frame(
                &mut renderer,
                &[] as &[SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>],
                Color32F::from([0.1, 0.1, 0.1, 1.0]),
                FrameFlags::empty(),
            ).expect("Initial render failed");
            compositor.queue_frame(()).expect("Initial queue failed");

            eprintln!("GPU ready, first frame queued for crtc {:?}", crtc);
            compositors.insert(crtc, compositor);
        }
    }

    state.gpu.insert(device_id, GpuData {
        fd: fd.device_fd(),
        drm,
        gbm,
        renderer,
        compositors,
    });
}

pub fn init_drm(event_loop: &mut EventLoop<'static, Treewm>, state: &mut Treewm) -> anyhow::Result<()> {
    eprintln!("Starting DRM init");
    let (session, notifier) = LibSeatSession::new()
        .map_err(|e| { eprintln!("Libseat error: {:?}", e); e })
        .expect("Failed to create libseat session");
    eprintln!("Session created");

    let handle = event_loop.handle();
    state.session = Some(session);
    handle.insert_source(notifier, |event, _, state| {
        match event {
            SessionEvent::PauseSession => {
                eprintln!("PauseSession fired");
                for gpu in state.gpu.values_mut() {
                    gpu.drm.pause();
                }
            }
            SessionEvent::ActivateSession => {
                eprintln!("ActivateSession fired");
                for gpu in state.gpu.values_mut() {
                    gpu.drm.activate(true).unwrap();
                    for compositor in gpu.compositors.values_mut() {
                        compositor.reset_buffers();
                        if let Err(e) = compositor.render_frame(
                            &mut gpu.renderer,
                            &[] as &[SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>],
                            Color32F::from([0.1, 0.1, 0.1, 1.0]),
                            FrameFlags::empty(),
                        ) { eprintln!("activate render_frame error: {:?}", e); }

                        if let Err(e) = compositor.queue_frame(()) {
                            eprintln!("activate queue_frame error: {:?}", e);
                        }
                    }
                }
            }
        }
    }).unwrap();

    let seat_name = state.session.as_ref().unwrap().seat();
    let udev_backend = UdevBackend::new(&seat_name).expect("Failed to initialize udev backend");
    eprintln!("Udev backend created");

    let existing: Vec<_> = udev_backend.device_list()
        .map(|(id, path)| (id, path.to_path_buf()))
        .collect();
    for (device_id, path) in existing {
        eprintln!("device_list entry: {:?}", path);
        if path.to_str().map(|s| s.starts_with("/dev/dri/card")).unwrap_or(false) {
            open_gpu(device_id, &path, state, &handle);
        }
    }

    let handle_inner = handle.clone();
    handle.insert_source(udev_backend, move |event, _, state| {
        if let UdevEvent::Added { device_id, path } = event {
            if path.to_str().map(|s| s.starts_with("/dev/dri/card")).unwrap_or(false) {
                open_gpu(device_id, &path, state, &handle_inner);
            }
        }
    }).unwrap();

    let interface = LibinputSessionInterface::from(state.session.as_ref().unwrap().clone());
    let mut context = Libinput::new_with_udev(interface);
    context.udev_assign_seat(&seat_name).unwrap();
    let libinput_backend = LibinputInputBackend::new(context.clone());

    eprintln!("calling input backend");
    handle.insert_source(libinput_backend, |event, _, state| {
        state.process_input_event(event);
    }).unwrap();
    eprintln!("input backend called");

    Ok(())
}
