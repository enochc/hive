use log::{debug, info};
use crate::signal::Signal;
use std::convert::TryFrom;
use std::fmt;
use std::borrow::Borrow;
use crate::hive::{PROPERTIES, PROPERTY, DELETE};
use std::collections::HashMap;
use async_std::task::block_on;
use bytes::{Bytes, BytesMut, BufMut};

pub type PropertyType = toml::Value;


#[derive(Default)]
pub struct Property
{
    name: Box<str>,
    pub value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}
impl fmt::Debug for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={:?}", self.name, self.value)
    }
}
impl Property {
    pub fn to_string(&self) -> String {
        return match &self.value {
            Some(t) => format!("{}={}", self.name, t.to_string()),
            None => format!("{}=None", self.name),
        }
    }
    pub fn from_table(table: &toml::value::Table) -> Option<Property>{
        if table.keys().len() != 1 {
            // return None
        }
        // for key in table.keys(){
        let key = table.keys().nth(0).unwrap();
        let val = table.get(key);
        let p = Property::from_toml(key.as_str(), val);
        return Some(p);


    }
    pub fn get_name(&self) -> &str {
        self.name.borrow()
    }
    pub fn new(name: &str, val: Option<PropertyType>) -> Property{
        return Property{
            name:Box::from(name),
            value: val,
            on_changed: Default::default()
        }
    }
    pub fn from_str(name: &str, val: &str) -> Property {
        Property::new(name, Some(PropertyType::from(val)))
    }
    pub fn from_bool(name: &str, val: bool) -> Property {
        Property::new(name, Some(PropertyType::from(val)))
    }
    pub fn from_float(name: &str, val: f64) -> Property {
        Property::new(name, Some(PropertyType::from(val)))
    }
    pub fn from_toml(name: &str, val:Option<&toml::Value>) -> Property{
        //Property::new(name, Some(PropertyType::from(val.to_string())))
        let p = match val {
            Some(v) if v.is_str() => {
                Property::from_str(name, v.as_str().unwrap())
            },
            Some(v) if v.is_integer() => {
                Property::from_int(name, v.as_integer().unwrap())
            },
            Some(v) if v.is_bool() => {
                Property::from_bool(name, v.as_bool().unwrap())
            },
            Some(v) if v.is_float() => {
                Property::from_float(name, v.as_float().unwrap())
            },
            _ => {
                println ! ("<<Failed to convert Property: {:?}", name);
                Property::new(name, None)
            }
        };
        return p;
    }
    pub fn from_int(name: &str, val: i64) -> Property {
        let small_int = u32::try_from(val);
        return match small_int {
            Ok(si) => {
                Property::new(name, Some(PropertyType::from(si)))
            },
            _ => {
                Property::new(name, Some(PropertyType::from(val)))
            }
        };
    }
    pub fn set_str(&mut self, s: &str){
        let p = PropertyType::from(s);
        self.set(p);
    }
    pub fn set_bool(&mut self, b: bool){
        let p = PropertyType::from(b);
        self.set(p);
    }
    pub fn set_int(&mut self, s: u32){
        let p = PropertyType::from(s);
        self.set(p);
    }
    pub fn set_float(&mut self, s: f64){
        let p = PropertyType::from(s);
        self.set(p);
    }

    pub fn set_from_prop(&mut self, v:Property)-> bool{
        return self.set(v.value.as_ref().unwrap().clone());
    }

    pub fn set(&mut self, v: PropertyType) -> bool
        where PropertyType: std::fmt::Debug + PartialEq + Sync + Send + Clone + 'static,
    {
        info!("<<<< set thing {}", v);
        let v_clone = v.clone();
        let op_v = Some(v);

        if !self.value.eq(&op_v) {
            self.value = op_v;
            info!("<<<< emit change!!");

            block_on(self.on_changed.emit(Some(v_clone)));
            return true;
        } else {
            debug!("do nothing ");
            return false;
        }
    }

    pub fn get(&self) -> &Option<PropertyType> {
        &self.value
    }
}
/*
    |P|one=1\ntwo=2\nthree=3
 */
pub(crate) fn properties_to_bytes(properties: &HashMap<String, Property>) -> Bytes {
    // let mut bytes = BytesMut::from(&[PROPERTIES]);
    let mut bytes = BytesMut::new();
    bytes.put_u8(PROPERTIES);
    // let mut message = PROPERTIES.to_string();
    for p in properties {
        if p.1.value.is_some() {
            // message.push_str(
            //     property_to_bytes(Some(p.1), false).unwrap().as_str()
            // );
            bytes.put_slice(
                &property_to_bytes(Some(p.1), false).unwrap()
            );
            // message.push('\n');
            bytes.put_u8(b'\n');
        }

    }
    return bytes.freeze()
}
/*
    |p|one=1
 */
pub(crate) fn property_to_bytes(property:Option<&Property>, inc_head:bool) -> Option<Bytes> {
    return match property {
        Some(p) if p.value.is_some() => {
            let prop_str = p.to_string();
            let bytes =  if inc_head {
                let mut b = BytesMut::with_capacity(prop_str.len()+1);
                b.put_u8(PROPERTY);
                b.put_slice(prop_str.as_bytes());
                b.freeze()
            } else {
                Bytes::from(prop_str)
            };
            Some(bytes)
        },
        Some(p) => {
            info!("... test2 {:?}", inc_head);
            let p_name = p.get_name();
            let mut bytes = BytesMut::with_capacity(p_name.len()+1);
            // let mut message = DELETE.to_string();
            bytes.put_u8(DELETE);
            // message.push_str(p.get_name());
            bytes.put_slice(p_name.as_bytes());
            Some(bytes.freeze())
        },
        _ => None
    }


}