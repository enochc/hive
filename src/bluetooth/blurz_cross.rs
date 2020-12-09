/// This mod only exists to allow writing and compiling code on my dev machine which happens
/// to be a mac. This code is never expected to run.

#[allow(dead_code)]
use std::error::Error;

pub struct Connection{}
#[derive(Debug)]
pub enum BtError {
    /// No specific information is known.
    Unknown,

    /// On Unix platforms: the error code and an explanation for this error code.
    Errno(u32, String),

    /// This error only has a description.
    Desc(String),

    /// `std::io::Error`
    IoError(std::io::Error),
}
impl From<std::io::Error> for BtError {
    fn from(error: std::io::Error) -> Self {
        BtError::IoError(error)
    }
}
impl std::fmt::Display for BtError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:}", std::error::Error::to_string(self))
    }
}
use std::str::FromStr;
impl Error for BtError {}
impl FromStr for BtAddr {
    type Err = ();
    fn from_str(_s: &str) -> Result<Self, Self::Err> { unimplemented!() }
}
#[derive(Clone)]
pub struct BluetoothAdapter<'a>{
    object_path: String,
    session: &'a BluetoothSession,
}
impl<'a> BluetoothAdapter<'a> {
    pub fn init(_session: &BluetoothSession) -> Result<BluetoothAdapter<'a>, Box<dyn Error>> {unimplemented!()}
    pub fn get_device_list(&self) -> Result<Vec<String>, Box<dyn Error>> {unimplemented!()}
    pub fn get_id(&self) -> String { unimplemented!() }
    pub fn set_discoverable(&self, _value: bool) -> Result<(), Box<dyn Error>> {unimplemented!()}
}
#[derive(Debug)]
pub struct BluetoothSession {

}
pub struct Message {
    // msg: *mut ffi::DBusMessage,
}

#[derive(Debug, Clone)]
pub struct ConnMsgs<C> {
    pub conn: C,
    pub timeout_ms: Option<u32>,
}
impl<C: std::ops::Deref<Target = Connection>> Iterator for ConnMsgs<C> {
    type Item = Message;
    fn next(&mut self) -> Option<Self::Item> {unimplemented!()}
}

impl BluetoothSession {
    pub fn create_session(_path: Option<&str>)->Result<BluetoothSession, Box<dyn Error>>{unimplemented!();}
    pub fn incoming(&self, _timeout_ms: u32) -> ConnMsgs<&Connection> {unimplemented!()}
}
#[derive(Clone, Debug)]
pub enum BluetoothEvent {
    Powered {
        object_path: String,
        powered: bool,
    },
    Discovering {
        object_path: String,
        discovering: bool,
    },
    Connected {
        object_path: String,
        connected: bool,
    },
    ServicesResolved {
        object_path: String,
        services_resolved: bool,
    },
    Value {
        object_path: String,
        value: Box<[u8]>,
    },
    RSSI {
        object_path: String,
        rssi: i16,
    },
    None,
}
impl BluetoothEvent {
    pub fn from(_conn_msg: Message) -> Option<BluetoothEvent> { unimplemented!() }
}

#[derive(Clone, Debug)]
pub struct BluetoothDevice<'a> {
    object_path: String,
    session: &'a BluetoothSession,
}
impl<'a> BluetoothDevice<'a>{
    pub fn new(_session: &'a BluetoothSession, _object_path: String) -> BluetoothDevice {unimplemented!()}
    pub fn is_connected(&self) -> Result<bool, Box<dyn Error>> {unimplemented!()}
    pub fn get_name(&self) -> Result<String, Box<dyn Error>> {unimplemented!()}
    pub fn get_address(&self) -> Result<String, Box<dyn Error>> {unimplemented!()}
    pub fn connect(&self, _timeout_ms: i32) -> Result<(), Box<dyn Error>> {unimplemented!()}
    pub fn get_gatt_services(&self) -> Result<Vec<String>, Box<dyn Error>> {unimplemented!()}
}

#[derive(Debug)]
pub struct BtSocketConnect<'a> {
    addr: BtAddr,
    // pollfd: RawFd,
    state: BtSocketConnectState,
    socket: &'a mut BtSocket,
    // query: QueryRFCOMMChannel,
}


#[derive(Debug)]
pub struct BtSocket{}
impl BtSocket{
    pub fn new(_proto: BtProtocol) -> Result<BtSocket, BtError> {unimplemented!()}
    pub fn connect(&mut self, _addr: BtAddr) -> Result<BtSocketConnect, BtError> {unimplemented!()}
    pub fn get_fd(&self) -> i32 {unimplemented!()}
}
#[derive(Debug)]
pub struct BtAddr{}

#[derive(Clone, Copy, Debug)]
pub enum BtProtocol {
    RFCOMM,
}

#[allow(dead_code)]
#[derive(Debug)]
enum BtSocketConnectState {
    SDPSearch,
    Connect,
    Done,
}

#[warn(dead_code)]
pub struct BluetoothDiscoverySession<'a> {
    _adapter: String,
    _session: &'a BluetoothSession,
}
impl<'a> BluetoothDiscoverySession<'a>{
    pub fn create_session(
        _session: &'a BluetoothSession,
        _adapter: String,
    ) -> Result<BluetoothDiscoverySession, Box<dyn Error>> {unimplemented!()}
    pub fn start_discovery(&self) -> Result<(), Box<dyn Error>> {unimplemented!()}
    pub fn stop_discovery(&self) -> Result<(), Box<dyn Error>> {unimplemented!()}
}

#[derive(Clone, Debug)]
pub struct BluetoothGATTService<'a> {
    object_path: String,
    session: &'a BluetoothSession,
}
impl<'a> BluetoothGATTService<'a> {
    pub fn new(_session: &'a BluetoothSession, _object_path: String) -> BluetoothGATTService {unimplemented!()}
    pub fn get_gatt_characteristics(&self) -> Result<Vec<String>, Box<dyn Error>> {unimplemented!()}
    pub fn get_uuid(&self) -> Result<String, Box<dyn Error>> {unimplemented!()}
}

#[derive(Clone, Debug)]
pub struct BluetoothGATTCharacteristic<'a> {
    object_path: String,
    session: &'a BluetoothSession,
}

impl<'a> BluetoothGATTCharacteristic<'a> {
    pub fn new(_session: &'a BluetoothSession, _object_path: String) -> BluetoothGATTCharacteristic {unimplemented!()}
    pub fn get_gatt_descriptors(&self) -> Result<Vec<String>, Box<dyn Error>> {unimplemented!()}
    pub fn read_value(&self, _offset: Option<u16>) -> Result<Vec<u8>, Box<dyn Error>> {unimplemented!()}
    pub fn write_value(&self, _values: Vec<u8>, _offset: Option<u16>) -> Result<(), Box<dyn Error>> {unimplemented!()}
    pub fn get_value(&self) -> Result<Vec<u8>, Box<dyn Error>> {unimplemented!()}
    pub fn get_uuid(&self) -> Result<String, Box<dyn Error>> {unimplemented!()}
    pub fn is_notifying(&self) -> Result<bool, Box<dyn Error>> {unimplemented!()}
    pub fn start_notify(&self) -> Result<(), Box<dyn Error>> {unimplemented!()}
    pub fn stop_notify(&self) -> Result<(), Box<dyn Error>> {unimplemented!()}
    pub fn get_flags(&self) -> Result<Vec<String>, Box<dyn Error>> {unimplemented!()}
}

#[derive(Clone, Debug)]
pub struct BluetoothGATTDescriptor<'a> {
    object_path: String,
    session: &'a BluetoothSession,
}
impl<'a> BluetoothGATTDescriptor<'a> {
    pub fn new(_session: &'a BluetoothSession, _object_path: String) -> BluetoothGATTDescriptor {unimplemented!()}
    pub fn read_value(&self, _offset: Option<u16>) -> Result<Vec<u8>, Box<dyn Error>> {unimplemented!()}
}
