use hyprland::data::{Clients, Monitors};
use hyprland::shared::{Address, HyprData, HyprDataVec};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::sync::mpsc;
use wayland_client::protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_hyprland::toplevel_export::v1::client::{
    hyprland_toplevel_export_frame_v1::{self, HyprlandToplevelExportFrameV1},
    hyprland_toplevel_export_manager_v1::HyprlandToplevelExportManagerV1,
};

pub struct CaptureRequest {
    pub ws_id: i32,
    pub monitor_name: String,
}

pub struct WindowThumbnail {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub x: i32,
    pub y: i32,
    pub win_width: i32,
    pub win_height: i32,
    pub address: Address,
}

pub struct CaptureResult {
    pub ws_id: i32,
    pub thumbnails: Vec<WindowThumbnail>,
    pub monitor_width: u32,
    pub monitor_height: u32,
}

struct CaptureState {
    shm: Option<wl_shm::WlShm>,
    export_manager: Option<HyprlandToplevelExportManagerV1>,
    frame_format: Option<wl_shm::Format>,
    frame_width: u32,
    frame_height: u32,
    frame_stride: u32,
    buffer_done: bool,
    frame_ready: bool,
    frame_failed: bool,
}

impl CaptureState {
    fn new() -> Self {
        Self {
            shm: None,
            export_manager: None,
            frame_format: None,
            frame_width: 0,
            frame_height: 0,
            frame_stride: 0,
            buffer_done: false,
            frame_ready: false,
            frame_failed: false,
        }
    }

    fn reset_frame(&mut self) {
        self.frame_format = None;
        self.frame_width = 0;
        self.frame_height = 0;
        self.frame_stride = 0;
        self.buffer_done = false;
        self.frame_ready = false;
        self.frame_failed = false;
    }
}

// Registry — bind wl_shm + hyprland_toplevel_export_manager_v1
impl Dispatch<wl_registry::WlRegistry, ()> for CaptureState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "hyprland_toplevel_export_manager_v1" => {
                    state.export_manager = Some(registry.bind(name, version.min(2), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm_pool::WlShmPool,
        _event: wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_buffer::WlBuffer,
        _event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<HyprlandToplevelExportManagerV1, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &HyprlandToplevelExportManagerV1,
        _event: <HyprlandToplevelExportManagerV1 as wayland_client::Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

// Frame events — Buffer, BufferDone, Ready, Failed
impl Dispatch<HyprlandToplevelExportFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _proxy: &HyprlandToplevelExportFrameV1,
        event: hyprland_toplevel_export_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            hyprland_toplevel_export_frame_v1::Event::Buffer {
                format: WEnum::Value(fmt),
                width,
                height,
                stride,
            } => {
                // Prefer Argb8888 or Xrgb8888
                if state.frame_format.is_none()
                    || fmt == wl_shm::Format::Argb8888
                    || fmt == wl_shm::Format::Xrgb8888
                {
                    state.frame_format = Some(fmt);
                    state.frame_width = width;
                    state.frame_height = height;
                    state.frame_stride = stride;
                }
            }
            hyprland_toplevel_export_frame_v1::Event::BufferDone => {
                state.buffer_done = true;
            }
            hyprland_toplevel_export_frame_v1::Event::Ready { .. } => {
                state.frame_ready = true;
            }
            hyprland_toplevel_export_frame_v1::Event::Failed => {
                state.frame_failed = true;
            }
            _ => {}
        }
    }
}

fn parse_window_handle(address: &str) -> Option<u32> {
    let hex = address.strip_prefix("0x").unwrap_or(address);
    u64::from_str_radix(hex, 16).ok().map(|v| v as u32)
}

fn capture_single_window(
    state: &mut CaptureState,
    event_queue: &mut EventQueue<CaptureState>,
    qh: &QueueHandle<CaptureState>,
    handle: u32,
) -> Option<(Vec<u8>, u32, u32, u32)> {
    let manager = state.export_manager.clone()?;
    let shm = state.shm.clone()?;

    state.reset_frame();

    let frame = manager.capture_toplevel(0, handle, qh, ());

    // Dispatch until BufferDone or Failed
    while !state.buffer_done && !state.frame_failed {
        if event_queue.blocking_dispatch(state).is_err() {
            frame.destroy();
            return None;
        }
    }

    if state.frame_failed || state.frame_format.is_none() {
        frame.destroy();
        return None;
    }

    let width = state.frame_width;
    let height = state.frame_height;
    let stride = state.frame_stride;
    let format = state.frame_format.unwrap();
    let buf_size = (stride * height) as usize;

    // Allocate shared memory via memfd
    let mfd = memfd::MemfdOptions::default().create("capture").ok()?;
    mfd.as_file().set_len(buf_size as u64).ok()?;

    let pool = shm.create_pool(mfd.as_file().as_fd(), buf_size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        format,
        qh,
        (),
    );

    // Reset ready/failed for the copy phase
    state.frame_ready = false;
    state.frame_failed = false;

    frame.copy(&buffer, 1);

    // Dispatch until Ready or Failed
    while !state.frame_ready && !state.frame_failed {
        if event_queue.blocking_dispatch(state).is_err() {
            frame.destroy();
            buffer.destroy();
            pool.destroy();
            return None;
        }
    }

    frame.destroy();

    if state.frame_failed {
        buffer.destroy();
        pool.destroy();
        return None;
    }

    // Read pixels from memfd
    let mut file = mfd.into_file();
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut data = vec![0u8; buf_size];
    file.read_exact(&mut data).ok()?;

    buffer.destroy();
    pool.destroy();

    Some((data, width, height, stride))
}

fn capture_workspace(
    state: &mut CaptureState,
    event_queue: &mut EventQueue<CaptureState>,
    qh: &QueueHandle<CaptureState>,
    ws_id: i32,
    monitor_name: &str,
) -> Option<CaptureResult> {
    let clients = Clients::get().ok()?;
    let monitors = Monitors::get().ok()?;

    let monitor = monitors
        .to_vec()
        .into_iter()
        .find(|m| m.name == monitor_name)?;

    let mon_x = monitor.x;
    let mon_y = monitor.y;
    let scale_factor = monitor.scale as f64;
    let monitor_width = (monitor.width as f64 / scale_factor) as u32;
    let monitor_height = (monitor.height as f64 / scale_factor) as u32;

    let ws_clients: Vec<_> = clients
        .to_vec()
        .into_iter()
        .filter(|c| c.workspace.id == ws_id && c.mapped && c.size.0 > 0 && c.size.1 > 0)
        .collect();

    if ws_clients.is_empty() {
        return None;
    }

    let mut thumbnails = Vec::new();

    for client in &ws_clients {
        let handle = match parse_window_handle(&client.address.to_string()) {
            Some(h) => h,
            None => continue,
        };

        if let Some((data, width, height, stride)) =
            capture_single_window(state, event_queue, qh, handle)
        {
            thumbnails.push(WindowThumbnail {
                data,
                width,
                height,
                stride,
                x: client.at.0 as i32 - mon_x,
                y: client.at.1 as i32 - mon_y,
                win_width: client.size.0 as i32,
                win_height: client.size.1 as i32,
                address: client.address.clone(),
            });
        }
    }

    if thumbnails.is_empty() {
        return None;
    }

    Some(CaptureResult {
        ws_id,
        thumbnails,
        monitor_width,
        monitor_height,
    })
}

pub fn spawn_capture_thread() -> (mpsc::Sender<CaptureRequest>, mpsc::Receiver<CaptureResult>) {
    let (req_tx, req_rx) = mpsc::channel::<CaptureRequest>();
    let (res_tx, res_rx) = mpsc::channel::<CaptureResult>();

    std::thread::spawn(move || {
        let conn = match Connection::connect_to_env() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("workspace_capture: failed to connect to wayland: {e}");
                return;
            }
        };

        let display = conn.display();
        let mut event_queue = conn.new_event_queue::<CaptureState>();
        let qh = event_queue.handle();
        let mut state = CaptureState::new();

        display.get_registry(&qh, ());

        if event_queue.roundtrip(&mut state).is_err() {
            eprintln!("workspace_capture: roundtrip failed");
            return;
        }

        if state.export_manager.is_none() {
            eprintln!("workspace_capture: hyprland_toplevel_export_manager_v1 not available");
            return;
        }
        if state.shm.is_none() {
            eprintln!("workspace_capture: wl_shm not available");
            return;
        }

        loop {
            let req = match req_rx.recv() {
                Ok(r) => r,
                Err(_) => return,
            };

            // Drain to latest request
            let mut latest = req;
            while let Ok(newer) = req_rx.try_recv() {
                latest = newer;
            }

            if let Some(result) = capture_workspace(
                &mut state,
                &mut event_queue,
                &qh,
                latest.ws_id,
                &latest.monitor_name,
            ) {
                let _ = res_tx.send(result);
            }
        }
    });

    (req_tx, res_rx)
}
