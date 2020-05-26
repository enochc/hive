use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::net::TcpListener;
use tokio::prelude::*;
use toml;

use crate::models::Property;

mod hive_macros;
pub mod models;
pub mod signal;

#[derive(Default, Clone)]
pub struct Hive {
    pub properties: HashMap<String, models::Property>
}

// #[derive(Deserialize)]
// struct ConfigProperty{
//     name: String,
//     // object_type: &'static str,
//     property_type: PropertyType,
//     default_value: Value,
// }
// impl Index<&str> for Hive {
//     type Output = Property;
//
//     fn index(&self, index: &str) -> &Self::Output {
//
//         &self.properties[index]
//     }
// }
// impl IndexMut<&str> for Hive {
//
//     fn index_mut(&mut self, index: &str) -> &mut self::Output {
//         // match index {
//         //     Side::Left => &self.left,
//         //     Side::Right => &self.right,
//         // }
//         &mut self.properties[index]
//     }
//
// }
impl Hive {
    pub fn get_mut_property(&mut self, key: &str) -> Option<&mut Property> {
        println!("properties: {:?}", self.properties.keys());
        let op = self.properties.get_mut(key);

        return op
    }

    pub fn parse_properties(toml: &toml::Value) -> Hive {

        let mut props:HashMap::<String, models::Property> = HashMap::new();
        let pp = toml.get("Properties").unwrap().as_table().unwrap();

        for key in pp.keys() {
            println!("<<<< key: {:?}",key);
            let val = pp.get(key);
            match val {
                Some(v) if v.is_str() => {
                    props.insert(String::from(key), Property::from_str(v.as_str().unwrap()));
                    // props[key] = ;
                },
                Some(v) if v.is_integer() => {
                    props.insert(String::from(key), Property::from_int(v.as_integer().unwrap()));
                },
                Some(v) if v.is_bool() => {
                    props.insert(String::from(key), Property::from_bool(v.as_bool().unwrap()));
                },
                _ => {
                    println!("<<Failed to Set Property: {:?}", key)
                }
            };
            println!("||{:?} == {:?}",key, val);
        }

        println!("properties (loaded): {:?}", props.keys());
        Hive {
            // properties:Default::default(),
            properties: props,
        }

    }

}

#[tokio::main]
pub async fn run(toml_path: &String) -> Result<bool, std::io::Error> {
    let foo: String = fs::read_to_string(toml_path).unwrap().parse().unwrap();
    let config: toml::Value = toml::from_str(&foo).unwrap();

    Hive::parse_properties(&config);
    let port = config.get("listen");
    if !port.is_none() {
        // listen on port
        match port.unwrap().as_integer() {
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
    }

    // start listening for incomming connections

    return Result::Ok(true)

}

