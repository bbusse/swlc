// SPDX-FileCopyrightText: Björn Busse <bj.rn@baerlin.eu>
// SPDX-License-Identifier: BSD-3-Clause
//
// Wayland wire protocol client — enumerates all toplevels (surfaces).
//
// Supported compositor protocols (at least one required):
//   zwlr-foreign-toplevel-management-v1  (wlroots / sway / hyprland)
//   ext-foreign-toplevel-list-v1         (KDE Plasma 6+, standardised)
//
// Protocol flow:
//   1. get_registry + sync(cb1)
//   2. On cb1.done: bind preferred manager + all wl_outputs + seat + sync(cb2)
//   3. On cb2.done: all initial toplevels and output properties received

use std::collections::HashMap;
use std::env;
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

// zwlr_foreign_toplevel_handle_v1 event opcodes:
//   0=title  1=app_id  2=output_enter  3=output_leave  4=state  5=done  6=closed  7=parent(v3)
//
// zwlr state values: 0=maximized  1=minimized  2=activated  3=fullscreen
//
// ext_foreign_toplevel_handle_v1 event opcodes:
//   0=closed  1=done  2=title  3=app_id  4=identifier
//
// wl_output event opcodes:
//   0=geometry(x,y,…)  1=mode(flags,w,h,refresh)  2=done  3=scale  4=name(v4)  5=description(v4)

#[derive(Clone, Copy, PartialEq, Eq)]
enum OT {
    None, Registry, Cb,
    MgrZwl, MgrExt,
    HdlZwl, HdlExt,
    Output,
}

pub struct Surface {
    pub app_id:      String,
    pub title:       String,
    pub identifier:  String,
    pub output_name: String,
    pub output_x:    i32,
    pub output_y:    i32,
    pub output_w:    i32,
    pub output_h:    i32,
    pub state:       Vec<String>,
}

struct OutputInfo {
    id:   u32,
    name: String,
    x:    i32,
    y:    i32,
    w:    i32,
    h:    i32,
}

struct Wl {
    stream:         UnixStream,
    rbuf:           Vec<u8>,
    next_id:        u32,
    obj_types:      HashMap<u32, OT>,
    tops:           Vec<Top>,
    outputs:        Vec<OutputInfo>,
    output_globals: Vec<(u32, u32)>, // (registry name, version)
    zwlr_name:      u32,
    zwlr_ver:       u32,
    ext_name:       u32,
    seat_name:      u32,
    seat_id:        u32,
    mgr_id:         u32,
    cb2:            u32,
    done:           bool,
    has_mgr:        bool,
}

struct Top {
    id:          u32,
    app:         String,
    title:       String,
    identifier:  String,
    output_id:   u32,
    state_flags: Vec<u32>,
    closed:      bool,
}

// Wire helpers

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_ne_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_ne_bytes([b[off], b[off + 1]])
}

// Wayland string: u32 length (including null) + bytes + null + padding to 4 bytes
fn wl_str(s: &str) -> Vec<u8> {
    let n = s.len() + 1;
    let padded = (n + 3) & !3;
    let mut v = Vec::with_capacity(4 + padded);
    v.extend_from_slice(&(n as u32).to_ne_bytes());
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v.resize(4 + padded, 0);
    v
}

// Parse a Wayland string at buf[off]; returns (string, bytes_consumed)
fn parse_str(buf: &[u8], off: usize) -> Option<(String, usize)> {
    if off + 4 > buf.len() { return None; }
    let n = rd_u32(buf, off) as usize;
    let padded = (n + 3) & !3;
    if off + 4 + padded > buf.len() { return None; }
    let s = String::from_utf8_lossy(
        &buf[off + 4..off + 4 + n.saturating_sub(1)]
    ).into_owned();
    Some((s, 4 + padded))
}

// Parse a Wayland array of uint32 at buf[off]
fn parse_array_u32(buf: &[u8], off: usize) -> Vec<u32> {
    if off + 4 > buf.len() { return vec![]; }
    let len = rd_u32(buf, off) as usize;
    (0..len / 4)
        .filter_map(|i| {
            let pos = off + 4 + i * 4;
            if pos + 4 <= buf.len() { Some(rd_u32(buf, pos)) } else { None }
        })
        .collect()
}

// Build a Wayland message: [obj_id u32][opcode u16][size u16][args...]
fn build_msg(obj: u32, op: u16, args: &[u8]) -> Vec<u8> {
    let sz = (8u16 + args.len() as u16).to_ne_bytes();
    let mut m = Vec::with_capacity(8 + args.len());
    m.extend_from_slice(&obj.to_ne_bytes());
    m.extend_from_slice(&op.to_ne_bytes());
    m.extend_from_slice(&sz);
    m.extend_from_slice(args);
    m
}

fn state_name(s: u32) -> &'static str {
    match s {
        0 => "maximized",
        1 => "minimized",
        2 => "activated",
        3 => "fullscreen",
        _ => "unknown",
    }
}

// Wl

impl Wl {
    fn connect(path: &str) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut w = Wl {
            stream,
            rbuf: Vec::with_capacity(64 * 1024),
            next_id: 4, // 1=display  2=registry  3=cb1
            obj_types: HashMap::new(),
            tops: Vec::new(),
            outputs: Vec::new(),
            output_globals: Vec::new(),
            zwlr_name: 0, zwlr_ver: 0,
            ext_name: 0,
            seat_name: 0, seat_id: 0,
            mgr_id: 0, cb2: 0,
            done: false, has_mgr: false,
        };
        w.obj_types.insert(2, OT::Registry);
        w.obj_types.insert(3, OT::Cb);
        Ok(w)
    }

    // Requests

    fn req_get_registry(&mut self) -> io::Result<()> {
        self.stream.write_all(&build_msg(1, 1, &2u32.to_ne_bytes()))
    }

    fn req_sync(&mut self, cb: u32) -> io::Result<()> {
        self.stream.write_all(&build_msg(1, 0, &cb.to_ne_bytes()))
    }

    fn req_activate(&mut self, handle_id: u32) -> io::Result<()> {
        // zwlr_foreign_toplevel_handle_v1.activate(seat) — opcode 4
        self.stream.write_all(&build_msg(handle_id, 4, &self.seat_id.to_ne_bytes()))
    }

    fn req_bind(&mut self, name: u32, iface: &str, ver: u32, nid: u32) -> io::Result<()> {
        let mut args = Vec::new();
        args.extend_from_slice(&name.to_ne_bytes());
        args.extend_from_slice(&wl_str(iface));
        args.extend_from_slice(&ver.to_ne_bytes());
        args.extend_from_slice(&nid.to_ne_bytes());
        self.stream.write_all(&build_msg(2, 0, &args))
    }

    // Receive

    fn recv_more(&mut self) -> io::Result<()> {
        let mut tmp = [0u8; 4096];
        loop {
            match self.stream.read(&mut tmp) {
                Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "compositor disconnected")),
                Ok(n) => { self.rbuf.extend_from_slice(&tmp[..n]); return Ok(()); }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    // Dispatch

    fn dispatch(&mut self) -> Result<usize, String> {
        if self.rbuf.len() < 8 { return Ok(0); }
        let obj_id = rd_u32(&self.rbuf, 0);
        let op     = rd_u16(&self.rbuf, 4);
        let sz     = rd_u16(&self.rbuf, 6) as usize;
        if sz < 8 { return Err(format!("bad message size {}", sz)); }
        if self.rbuf.len() < sz { return Ok(0); }

        let args: Vec<u8> = self.rbuf[8..sz].to_vec();
        self.handle_event(obj_id, op, &args).map_err(|e| e.to_string())?;
        Ok(sz)
    }

    fn handle_event(&mut self, obj_id: u32, op: u16, args: &[u8]) -> io::Result<()> {
        let t = self.obj_types.get(&obj_id).copied().unwrap_or(OT::None);

        if obj_id == 1 && op == 0 {
            let msg = parse_str(args, 8).map(|(s, _)| s).unwrap_or_default();
            return Err(io::Error::new(ErrorKind::Other, format!("compositor error: {}", msg)));
        }

        match (t, op) {
            (OT::Registry, 0) if args.len() >= 8 => {
                let name = rd_u32(args, 0);
                if let Some((iface, cs)) = parse_str(args, 4) {
                    let ver = if 4 + cs + 4 <= args.len() { rd_u32(args, 4 + cs) } else { 0 };
                    match iface.as_str() {
                        "zwlr_foreign_toplevel_management_v1" if self.zwlr_name == 0 => {
                            self.zwlr_name = name; self.zwlr_ver = ver;
                        }
                        "ext_foreign_toplevel_list_v1" if self.ext_name == 0 => {
                            self.ext_name = name;
                        }
                        "wl_seat" if self.seat_name == 0 => {
                            self.seat_name = name;
                        }
                        "wl_output" => {
                            self.output_globals.push((name, ver));
                        }
                        _ => {}
                    }
                }
            }
            (OT::Cb, 0) => {
                if obj_id == 3 {
                    // Bind toplevel manager
                    if self.zwlr_name != 0 {
                        let nid = self.next_id; self.next_id += 1;
                        let (name, ver) = (self.zwlr_name, self.zwlr_ver.min(3));
                        self.mgr_id = nid;
                        self.obj_types.insert(nid, OT::MgrZwl);
                        self.req_bind(name, "zwlr_foreign_toplevel_management_v1", ver, nid)?;
                        self.has_mgr = true;
                    } else if self.ext_name != 0 {
                        let nid = self.next_id; self.next_id += 1;
                        let name = self.ext_name;
                        self.mgr_id = nid;
                        self.obj_types.insert(nid, OT::MgrExt);
                        self.req_bind(name, "ext_foreign_toplevel_list_v1", 1, nid)?;
                        self.has_mgr = true;
                    }
                    // Bind seat (needed for activate)
                    if self.seat_name != 0 {
                        let nid = self.next_id; self.next_id += 1;
                        self.seat_id = nid;
                        self.req_bind(self.seat_name, "wl_seat", 1, nid)?;
                    }
                    // Bind all wl_outputs; their name/geometry/mode events arrive before cb2
                    for (name, ver) in self.output_globals.clone() {
                        let nid = self.next_id; self.next_id += 1;
                        self.obj_types.insert(nid, OT::Output);
                        self.outputs.push(OutputInfo {
                            id: nid, name: String::new(),
                            x: 0, y: 0, w: 0, h: 0,
                        });
                        self.req_bind(name, "wl_output", ver.min(4), nid)?;
                    }
                    let cb2 = self.next_id; self.next_id += 1;
                    self.cb2 = cb2;
                    self.obj_types.insert(cb2, OT::Cb);
                    self.req_sync(cb2)?;
                } else if obj_id == self.cb2 {
                    self.done = true;
                }
            }
            // wl_output events
            (OT::Output, 0) if args.len() >= 8 => {
                // geometry: x(i32) y(i32) phys_w phys_h subpixel make model transform
                let x = rd_u32(args, 0) as i32;
                let y = rd_u32(args, 4) as i32;
                if let Some(o) = self.outputs.iter_mut().find(|o| o.id == obj_id) {
                    o.x = x; o.y = y;
                }
            }
            (OT::Output, 1) if args.len() >= 12 => {
                // mode: flags(u32) width(i32) height(i32) refresh(i32)
                // only record the current mode (flags & 1)
                let flags = rd_u32(args, 0);
                if flags & 1 != 0 {
                    let w = rd_u32(args, 4) as i32;
                    let h = rd_u32(args, 8) as i32;
                    if let Some(o) = self.outputs.iter_mut().find(|o| o.id == obj_id) {
                        o.w = w; o.h = h;
                    }
                }
            }
            (OT::Output, 4) => {
                // name (wl_output v4)
                if let Some((s, _)) = parse_str(args, 0) {
                    if let Some(o) = self.outputs.iter_mut().find(|o| o.id == obj_id) {
                        o.name = s;
                    }
                }
            }
            (OT::MgrZwl, 0) if args.len() >= 4 => {
                let nid = rd_u32(args, 0);
                self.obj_types.insert(nid, OT::HdlZwl);
                self.tops.push(Top {
                    id: nid, app: String::new(), title: String::new(),
                    identifier: String::new(), output_id: 0,
                    state_flags: Vec::new(), closed: false,
                });
            }
            (OT::MgrExt, 0) if args.len() >= 4 => {
                let nid = rd_u32(args, 0);
                self.obj_types.insert(nid, OT::HdlExt);
                self.tops.push(Top {
                    id: nid, app: String::new(), title: String::new(),
                    identifier: String::new(), output_id: 0,
                    state_flags: Vec::new(), closed: false,
                });
            }
            (OT::HdlZwl, _) => {
                if let Some(tp) = self.tops.iter_mut().find(|t| t.id == obj_id) {
                    match op {
                        0 => { if let Some((s, _)) = parse_str(args, 0) { tp.title = s; } }
                        1 => { if let Some((s, _)) = parse_str(args, 0) { tp.app   = s; } }
                        2 if args.len() >= 4 => { tp.output_id = rd_u32(args, 0); }
                        3 if args.len() >= 4 => {
                            if tp.output_id == rd_u32(args, 0) { tp.output_id = 0; }
                        }
                        4 => { tp.state_flags = parse_array_u32(args, 0); }
                        6 => { tp.closed = true; }
                        _ => {}
                    }
                }
            }
            (OT::HdlExt, _) => {
                if let Some(tp) = self.tops.iter_mut().find(|t| t.id == obj_id) {
                    match op {
                        0 => { tp.closed = true; }
                        2 => { if let Some((s, _)) = parse_str(args, 0) { tp.title      = s; } }
                        3 => { if let Some((s, _)) = parse_str(args, 0) { tp.app        = s; } }
                        4 => { if let Some((s, _)) = parse_str(args, 0) { tp.identifier = s; } }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn run_loop(&mut self) -> Result<(), String> {
        while !self.done {
            loop {
                match self.dispatch() {
                    Ok(0)  => break,
                    Ok(n)  => { self.rbuf.drain(..n); }
                    Err(e) => return Err(e),
                }
                if self.done { break; }
            }
            if !self.done {
                self.recv_more().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

// Public API

pub fn list_surfaces() -> Result<Vec<Surface>, String> {
    let rtdir = env::var("XDG_RUNTIME_DIR")
        .map_err(|_| "XDG_RUNTIME_DIR not set".to_string())?;
    let disp = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let path = if disp.starts_with('/') { disp } else { format!("{}/{}", rtdir, disp) };

    let mut wl = Wl::connect(&path).map_err(|e| format!("connect: {}", e))?;
    wl.req_get_registry().and_then(|_| wl.req_sync(3))
        .map_err(|e| e.to_string())?;
    wl.run_loop()?;

    if !wl.has_mgr {
        return Err(
            "no surface-listing protocol available; compositor needs \
             zwlr-foreign-toplevel-management-v1 or ext-foreign-toplevel-list-v1"
                .into(),
        );
    }

    Ok(wl.tops.into_iter()
        .filter(|t| !t.closed)
        .map(|t| {
            let out = wl.outputs.iter().find(|o| o.id == t.output_id);
            Surface {
                app_id:      t.app,
                title:       t.title,
                identifier:  t.identifier,
                output_name: out.map(|o| o.name.clone()).unwrap_or_default(),
                output_x:    out.map(|o| o.x).unwrap_or(0),
                output_y:    out.map(|o| o.y).unwrap_or(0),
                output_w:    out.map(|o| o.w).unwrap_or(0),
                output_h:    out.map(|o| o.h).unwrap_or(0),
                state:       t.state_flags.iter().map(|&s| state_name(s).to_string()).collect(),
            }
        })
        .collect())
}

// Activate (focus) the first surface whose app_id or title matches `target`.
// Only works with zwlr_foreign_toplevel_management_v1 (wlroots compositors).
pub fn activate(target: &str) -> Result<(), String> {
    let rtdir = env::var("XDG_RUNTIME_DIR")
        .map_err(|_| "XDG_RUNTIME_DIR not set".to_string())?;
    let disp = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let path = if disp.starts_with('/') { disp } else { format!("{}/{}", rtdir, disp) };

    let mut wl = Wl::connect(&path).map_err(|e| format!("connect: {}", e))?;
    wl.req_get_registry().and_then(|_| wl.req_sync(3))
        .map_err(|e| e.to_string())?;
    wl.run_loop()?;

    if !wl.has_mgr {
        return Err("no surface-listing protocol available".into());
    }
    if wl.seat_id == 0 {
        return Err("no wl_seat available".into());
    }

    let handle_id = wl.tops.iter()
        .find(|t| !t.closed && (t.app == target || t.title == target))
        .map(|t| t.id)
        .ok_or_else(|| format!("no surface matching '{}'", target))?;

    wl.req_activate(handle_id).map_err(|e| e.to_string())?;

    // Flush with a sync so the compositor processes the activate before we disconnect
    let cb = wl.next_id;
    wl.req_sync(cb).map_err(|e| e.to_string())?;

    Ok(())
}
