use crate::signal::Signal;
use serde_derive::Deserialize;
use std::convert::TryFrom;

#[derive(PartialEq, Clone, Debug, Deserialize)]
pub enum PropertyType {
    REAL(i64),
    INT(u32),
    BOOL(bool),
    STRING(Box<str>),
}

#[derive(Default, Clone)]
pub struct Property
{
    value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}

impl Property {
    pub fn from_str(val: &str) -> Property {
            return Property{
            value: Some(PropertyType::STRING(String::from(val).into_boxed_str())),
            on_changed: Default::default(),
        };
    }
    pub fn from_int(val: i64) -> Property {
        let small_int = u32::try_from(val);
        return match small_int {
            Ok(si) => {
                Property {
                    value: Some(PropertyType::INT(si)),
                    on_changed: Default::default(),
                }
            },
            _ => {
                Property {
                    value: Some(PropertyType::REAL(val)),
                    on_changed: Default::default(),
                }
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

    fn set(&mut self, v: PropertyType)
        where PropertyType: std::fmt::Debug + PartialEq + Sync + Send + Clone + 'static,
    {

        let v_clone = v.clone();
        let op_v = Some(v);

        if !self.value.eq(&op_v) {
            self.value = op_v;
            self.on_changed.emit(Some(v_clone));
        } else {
            println!("do nothing ")
        }
    }

    pub fn get(&self) -> &Option<PropertyType> {
        &self.value
    }
}