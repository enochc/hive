use core::fmt::Debug;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Error;
use std::fmt::Formatter;
use std::thread::sleep;
use std::io::prelude::*;
use std::io::{self, Read};

use failure::_core::any::Any;
use failure::_core::hash::Hash;
use failure::_core::ptr::null;
use failure::_core::time::Duration;
use futures::executor::block_on;
use serde_derive::Deserialize;
use tokio::prelude::*;

use {
    config::Config,
    //tokio::net::tcp::Incoming,
    //tokio::codec::{Framed, LinesCodec},
    config::Value,
    futures::future,
    futures::future::Future,
    futures::future::lazy,
    //failure::{format_err, Error},
    futures::stream::Stream,
    std::net::{IpAddr, Ipv4Addr, SocketAddr},
    tokio::net::{TcpListener, TcpStream},
    //std::sync::{Arc, Mutex, RwLock},
};

use crate::models::{Property, PropertyType};
use std::thread;
use futures::TryFutureExt;

use toml;
use std::fs;
use toml::value::Value::Table;
// use tokio::sync::mpsc::block::Read::Value;
// use futures::io::Result;

// use tokio::future::ok;

// #[macro_use]
mod hive_macros;
pub mod models;
pub mod signal;

#[derive(Default)]
pub struct Hive {
    properties: HashMap<String, models::Property<PropertyType>>
}

#[derive(Deserialize)]
struct ConfigProperty{
    name: String,
    // object_type: &'static str,
    property_type: PropertyType,
    default_value: Value,
}

impl Hive {
    async fn parse_properties(toml: &toml::Value) -> Self {

        // let pros:HashMap::<String, models::Property<PropertyType>> = Default::default();
        //
        // let pp = toml.get("Properties").unwrap().as_table().unwrap();
        //
        // let mut properties: HashMap<String, models::Property<PropertyType>> = Default::default();
        // for key in pp.keys() {
        //
        //     let val = pp.get(key);
        //     let p = match val {
        //         Some(v) if v.is_str() => {
        //             properties[key] = Property::new(v.as_str().unwrap());
        //         },
        //         //_ => Default::default(),
        //     };
        //     println!("||{:?} == {:?}, Property: {:?}",key, val, p);
        // }
        let props:HashMap<String, Property<PropertyType>> = HashMap::new();
        Hive {
            // properties:Default::default(),
            properties: props,
        }

    }

}

#[tokio::main]
pub async fn run(tomlPath: &String) -> Result<bool, std::io::Error> {
    let foo: String = fs::read_to_string(tomlPath).unwrap().parse().unwrap();
    let config: toml::Value = toml::from_str(&foo).unwrap();

    Hive::parse_properties(&config).await;
    let port = config.get("listen").unwrap().as_integer();

    // start listening for incomming connections
    match port {
        Some(port) => {
            let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port as u16);
            println!("Listening for connections on {:?}", addr);
            let mut listener = TcpListener::bind(addr).await?;

            loop {
                let (mut socket, _) = listener.accept().await?;
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    // In a loop, read data from the socket and write the data back.
                    loop {
                        let n = match socket.read(&mut buf).await {
                            // socket closed
                            Ok(n) if n == 0 => return,
                            Ok(n) => n,
                            Err(e) => {
                                eprintln!("failed to read from socket; err = {:?}", e);
                                return;
                            }
                        };
                        println!("RECEIVED: {:?}", &buf[0..n]);
                    }
                });
            }
        }
        _ => {
            println!("No listen port specified, not listening");
        }
    }
    return Result::Ok(true)

}

