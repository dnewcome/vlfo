//! OSC receiver: `/vlfo/<input> <values...>` sets an ISF input.
//! Runs on its own thread and writes into the shared parameter store.

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rosc::{OscMessage, OscPacket, OscType};

use crate::params::Params;

pub const PREFIX: &str = "/vlfo/";

pub fn spawn(port: u16, params: Arc<Mutex<Params>>) -> Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", port))
        .with_context(|| format!("cannot bind OSC UDP port {port}"))?;
    log::info!("OSC listening on udp port {port}, addresses {PREFIX}<input>");
    std::thread::Builder::new()
        .name("vlfo-osc".into())
        .spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                let Ok((n, _from)) = socket.recv_from(&mut buf) else { continue };
                match rosc::decoder::decode_udp(&buf[..n]) {
                    Ok((_, packet)) => handle(packet, &params),
                    Err(e) => log::warn!("bad OSC packet: {e}"),
                }
            }
        })?;
    Ok(())
}

fn handle(packet: OscPacket, params: &Arc<Mutex<Params>>) {
    match packet {
        OscPacket::Message(m) => apply(m, params),
        OscPacket::Bundle(b) => {
            for p in b.content {
                handle(p, params);
            }
        }
    }
}

fn to_f32(a: &OscType) -> Option<f32> {
    match a {
        OscType::Float(f) => Some(*f),
        OscType::Double(d) => Some(*d as f32),
        OscType::Int(i) => Some(*i as f32),
        OscType::Long(i) => Some(*i as f32),
        OscType::Bool(b) => Some(*b as i32 as f32),
        OscType::Nil => Some(1.0), // a bare bang
        _ => None,
    }
}

fn apply(m: OscMessage, params: &Arc<Mutex<Params>>) {
    let Some(name) = m.addr.strip_prefix(PREFIX) else {
        log::debug!("ignoring OSC address {}", m.addr);
        return;
    };
    let mut vals: Vec<f32> = m.args.iter().filter_map(to_f32).collect();
    if vals.is_empty() {
        vals.push(1.0); // message with no args = trigger
    }
    let ok = params.lock().unwrap().set_floats(name, &vals);
    if ok {
        log::debug!("{} <- {:?}", m.addr, vals);
    } else {
        log::debug!("unknown input {name}");
    }
}
