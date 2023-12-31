#![cfg_attr(not(test), no_std)]

#![doc = include_str!("../README.md")]

use core::{convert::Infallible, marker::PhantomData};

pub trait ConfigurationMarshal<Config>
{
  type Error;
  fn write(&mut self, config: &Config) -> Result<(), Self::Error>;
  fn read(&mut self) -> Result<Config, Self::Error>;
}

pub trait GenericConfigurationProvider<Config>
where
  Config: Clone
{
  fn commit(&mut self) -> bool;
  fn current(&self) -> &Config;
  fn current_mut(&mut self) -> &mut Config;
}

pub struct ConfigurationProvider<Config, Marshal> {
  volatile: Config,
  marshal: Marshal
}

impl<Config, Marshal> ConfigurationProvider<Config, Marshal>
where
  Config: Default + Clone,
  Marshal: ConfigurationMarshal<Config>
{
  pub fn new(mut marshal: Marshal) -> Result<Self, Marshal::Error> {
    let current = marshal.read();
    match current {
      Ok(c) => {
        Ok(Self { marshal, volatile: c })
      },
      Err(_) => {
        let c = Config::default();
        marshal.write(&c)?;
        Ok(Self { marshal, volatile: c })
      },
    }
  }
}

impl<Config, Marshal> GenericConfigurationProvider<Config> for ConfigurationProvider<Config, Marshal>
where
  Config: Default + Clone,
  Marshal: ConfigurationMarshal<Config>
{
  fn commit(&mut self) -> bool {
    self.marshal.write(&self.volatile).is_ok()
  }

  fn current(&self) -> &Config {
    &self.volatile
  }

  fn current_mut(&mut self) -> &mut Config {
    &mut self.volatile
  }
}

pub struct VolatileMarshal<Config>(PhantomData<Config>);

impl<Config> VolatileMarshal<Config> {
  pub fn new() -> Self {
    Self(PhantomData)
  }
}

impl<Config> ConfigurationMarshal<Config> for VolatileMarshal<Config>
where
  Config: Default
{
  type Error = Infallible;

  fn write(&mut self, _config: &Config) -> Result<(), Self::Error> {
    Ok(())
  }

  fn read(&mut self) -> Result<Config, Self::Error> {
    Ok(Config::default())
  }
}

#[cfg(feature = "m24c64")]
pub mod m24c64 {
  extern crate alloc;

  use core::marker::PhantomData;

  use binmarshal::{rw::{VecBitWriter, BitWriter, BitView}, Demarshal, DemarshalOwned, Marshal};
  use embedded_hal::blocking::{i2c, delay::DelayMs};
  use grapple_m24c64::M24C64;
  use alloc::vec;

  use crate::ConfigurationMarshal;

  pub struct M24C64ConfigurationMarshal<Config, I2C, Delay> {
    delay: Delay,
    address_offset: usize,
    eeprom: M24C64<I2C>,
    marker: PhantomData<Config>
  }

  pub enum M24C64ConfigurationError<E> {
    Serialisation,
    I2C(E),
    BlankEeprom
  }

  impl<Config, I2C, Delay> M24C64ConfigurationMarshal<Config, I2C, Delay> {
    #[allow(unused)]
    pub fn new(eeprom: M24C64<I2C>, address: usize, delay: Delay, marker: PhantomData<Config>) -> Self {
      Self { delay, address_offset: address, eeprom, marker }
    }
  }

  impl<'a, I2C, Delay, Config, E> ConfigurationMarshal<Config> for M24C64ConfigurationMarshal<Config, I2C, Delay>
  where
    Config: Marshal<()> + DemarshalOwned + Default + Clone,
    I2C: i2c::Write<u8, Error = E> + i2c::WriteRead<u8, Error = E>,
    Delay: DelayMs<u16>
  {
    type Error = M24C64ConfigurationError<E>;

    fn write(&mut self, config: &Config) -> Result<(), Self::Error> {
      // let bytes = config.to_bytes().map_err(|e| Self::Error::Deku(e))?;
      let mut writer = VecBitWriter::new();
      if config.clone().write(&mut writer, ()).is_err() {
        return Err(Self::Error::Serialisation);
      }
      let bytes = writer.slice();
      self.eeprom.write(self.address_offset, &(bytes.len() as u16).to_le_bytes(), &mut self.delay).map_err(|e| Self::Error::I2C(e))?;
      self.delay.delay_ms(10u16);
      self.eeprom.write(self.address_offset + 0x02, &bytes[..], &mut self.delay).map_err(|e| Self::Error::I2C(e))?;
      Ok(())
    }

    fn read(&mut self) -> Result<Config, Self::Error> {
      let mut len_buf = [0u8; 2];
      self.eeprom.read(self.address_offset, &mut len_buf[..]).map_err(|e| Self::Error::I2C(e))?;

      if len_buf[0] == 255 && len_buf[1] == 255 {
        return Err(Self::Error::BlankEeprom);
      }

      let mut buf = vec![0u8; u16::from_le_bytes(len_buf) as usize];
      self.eeprom.read(self.address_offset + 0x02, &mut buf[..]).map_err(|e| Self::Error::I2C(e))?;
      match Config::read(&mut BitView::new(&buf), ()) {
        Ok(c) => Ok(c),
        Err(_) => Err(Self::Error::Serialisation),
      }
    }
  }
}