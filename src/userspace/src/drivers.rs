// CosinusOS Userspace — drivers.rs
// Driver trait + DriverManager

use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::syscall::{print, println};

pub trait Driver {
    fn name(&self)                     -> &str;
    fn init(&mut self)                 -> Result<(), ()>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()>;
    fn write(&mut self, buf: &[u8])    -> Result<usize, ()>;
}

pub struct DriverManager {
    drivers: Vec<Box<dyn Driver>>,
}

impl DriverManager {
    pub fn new() -> Self { Self { drivers: Vec::new() } }

    pub fn register(&mut self, driver: Box<dyn Driver>) {
        self.drivers.push(driver);
    }

    pub fn init_all(&mut self) {
        for driver in &mut self.drivers {
            match driver.init() {
                Ok(_)  => { print("Driver "); print(driver.name()); println(" initialized"); }
                Err(_) => { print("Driver "); print(driver.name()); println(" failed!"); }
            }
        }
    }

    pub fn get(&mut self, name: &str) -> Option<&mut Box<dyn Driver>> {
        self.drivers.iter_mut().find(|d| d.name() == name)
    }
}
