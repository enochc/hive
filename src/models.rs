use crate::signal::Signal;

#[derive(PartialEq, Clone, Debug, Deserialize)]
pub enum PropertyType {
    REAL(i64),
    INT(u32),
    SHORT(u8),
    BOOL(bool),
    STRING(Box<str>),
}

#[derive(Default)]
pub struct Property<PropertyType>
{
    value: Option<PropertyType>,
    pub on_changed: Signal<Option<PropertyType>>,
}

impl<PropertyType: Clone> Property<PropertyType> {
    pub fn set(&mut self, v: PropertyType)
        where PropertyType: std::fmt::Debug + PartialEq + Send + Clone + 'static,
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