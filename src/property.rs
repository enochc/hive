use crate::signal::Signal;
use std::convert::TryFrom;
use futures::executor::block_on;
use std::fmt;
use std::borrow::Borrow;
use crate::hive::{PROPERTIES, PROPERTY, DELETE};
use std::collections::HashMap;

// #[derive(PartialEq, Clone, Debug, Deserialize, Serialize)]
// pub enum PropertyType {
//     REAL(i64),
//     FLOAT(f64),
//     INT(u32),
//     BOOL(bool),
//     STRING(Box<str>),
// }

pub type PropertyType = toml::Value;

#[derive(Default)]
pub struct Property
{
    name: Box<str>,
    pub value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}
impl fmt::Debug for Property {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
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
    pub fn emit(&mut self){
        block_on(self.on_changed.emit(None));
    }

    pub fn set(&mut self, v: PropertyType) -> bool
        where PropertyType: std::fmt::Debug + PartialEq + Sync + Send + Clone + 'static,
    {
        println!("<<<< set thing {}", v);
        let v_clone = v.clone();
        let op_v = Some(v);

        if !self.value.eq(&op_v) {
            self.value = op_v;
            println!("<<<< emit change!!");
            block_on(self.on_changed.emit(Some(v_clone)));
            return true;
        } else {
            println!("do nothing ");
            return false;
        }
    }

    pub fn get(&self) -> &Option<PropertyType> {
        &self.value
    }
}

pub(crate) fn properties_to_sock_str(properties: &HashMap<String, Property>) -> String {
    let mut message = PROPERTIES.to_string();
    for p in properties {
        message.push_str(
            property_to_sock_str(Some(p.1), false).unwrap().as_str()
        );
        message.push('\n');
    }
    return String::from(message);
}

pub(crate) fn property_to_sock_str(property:Option<&Property>, inc_head:bool) -> Option<String> {
    return match property {
        Some(p) if p.value.is_some() => {
            let mut message = if inc_head {
                PROPERTY.to_string()
            } else {
                String::from("")
            };
            message.push_str(p.to_string().as_str());
            Some(message)
        },
        Some(p) => {
            let mut message = DELETE.to_string();
            message.push_str(p.get_name());
            Some(message)
        },
        _ => None
    }


}