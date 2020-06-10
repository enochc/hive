use crate::signal::Signal;
use serde_derive::Deserialize;
use std::convert::TryFrom;
use futures::executor::block_on;
use std::fmt;
use std::borrow::Borrow;

//TODO I may not need Property at all, but just PropertyType renamed to propert,
// instead of all the from methods, I could have as methods and/or just use serde
#[derive(PartialEq, Clone, Debug, Deserialize)]
pub enum PropertyType {
    REAL(i64),
    FLOAT(f64),
    INT(u32),
    BOOL(bool),
    STRING(Box<str>),
}

#[derive(Default)]
pub struct Property
{
    name: Box<str>,
    pub value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}
impl fmt::Debug for Property {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({:?}, {:?})", self.name, self.value)
    }
}
impl Property {
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
        Property::new(name, Some(PropertyType::STRING(String::from(val).into_boxed_str())))
    }
    pub fn from_bool(name: &str, val: bool) -> Property {
        Property::new(name, Some(PropertyType::BOOL(val)))
    }
    pub fn from_float(name: &str, val: f64) -> Property {
        Property::new(name, Some(PropertyType::FLOAT(val)))
    }
    pub fn from_int(name: &str, val: i64) -> Property {
        let small_int = u32::try_from(val);
        return match small_int {
            Ok(si) => {
                Property::new(name, Some(PropertyType::INT(si)))
            },
            _ => {
                Property::new(name, Some(PropertyType::REAL(val)))
            }
        };
    }
    pub fn set_str(&mut self, s: &str){
        let p = PropertyType::STRING(String::from(s).into_boxed_str());
        self.set(p);
    }
    pub fn set_bool(&mut self, b: bool){
        let p = PropertyType::BOOL(b);
        self.set(p);
    }
    pub fn set_int(&mut self, s: u32){
        let p = PropertyType::INT(s);
        self.set(p);
    }
    pub fn set_float(&mut self, s: f64){
        let p = PropertyType::FLOAT(s);
        self.set(p);
    }

    pub fn set_from_prop(&mut self, v:&Property){
        self.set(v.value.as_ref().unwrap().clone())
    }

    pub fn set(&mut self, v: PropertyType)
        where PropertyType: std::fmt::Debug + PartialEq + Sync + Send + Clone + 'static,
    {

        let v_clone = v.clone();
        let op_v = Some(v);

        if !self.value.eq(&op_v) {
            self.value = op_v;
            block_on(self.on_changed.emit(Some(v_clone)));
        } else {
            println!("do nothing ")
        }
    }

    pub fn get(&self) -> &Option<PropertyType> {
        &self.value
    }
}