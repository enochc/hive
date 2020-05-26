use crate::signal::Signal;
use serde_derive::Deserialize;

#[derive(PartialEq, Clone, Debug, Deserialize)]
pub enum PropertyType {
    REAL(i64),
    INT(u32),
    BOOL(bool),
    STRING(Box<str>),
}

#[derive(Default)]
pub struct Property<PropertyType>
{
    value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}

impl Property<PropertyType> {
    pub fn new(val: &str) -> Property<PropertyType> {
            return Property{
            value: Some(PropertyType::STRING(String::from(val).into_boxed_str())),
            on_changed: Default::default(),
        };
    }
    pub fn set(&mut self, v: PropertyType)
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