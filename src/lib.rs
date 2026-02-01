#![cfg_attr(not(test), no_std)]

#![doc = include_str!("../README.md")]

use core::marker::PhantomData;

#[derive(Debug)]
pub enum ConfigurationError {
  BusError,
  SerialisationError,
  BlankError
}

impl ConfigurationError {
  fn can_retry(&self) -> bool {
    match self {
      ConfigurationError::BusError => true,
      ConfigurationError::SerialisationError => false,
      ConfigurationError::BlankError => false
    }
  }
}

pub trait ConfigurationMarshal<Config>
{
  type Error: Into<ConfigurationError>;

  fn write(&mut self, config: &Config) -> Result<(), Self::Error>;
  fn read(&mut self) -> Result<Config, Self::Error>;

  fn write_with_retry(&mut self, config: &Config, retries: usize) -> Result<(), ConfigurationError> {
    let mut i = 0;

    while let Err(e) = self.write(&config) {
      let config_err = e.into();
      if !config_err.can_retry() {
        return Err(config_err)
      }

      i = i + 1;
      if i >= retries {
        return Err(config_err)
      }
    }

    return Ok(());
  }

  fn read_with_retry(&mut self, retries: usize) -> Result<Config, ConfigurationError> {
    let mut i = 0;

    loop {
      match self.read() {
        Ok(cfg) => {
          return Ok(cfg)
        },
        Err(e) => {
          let config_err = e.into();
          if !config_err.can_retry() {
            return Err(config_err)
          }

          i = i + 1;
          if i >= retries {
            return Err(config_err)
          }
        }
      }
    }
  }
}

pub trait GenericConfigurationProvider<Config>
where
  Config: Clone
{
  fn current(&self) -> &Config;
  fn current_mut(&mut self) -> &mut Config;

  fn commit(&mut self, reload: bool) -> Result<(), ConfigurationError>;
  fn reload(&mut self) -> Result<(), ConfigurationError>;
}

pub struct ConfigurationProvider<Config, Marshal> {
  volatile: Config,
  marshal: Marshal,
  max_retries: usize,
}

impl<Config, Marshal> ConfigurationProvider<Config, Marshal>
where
  Config: Default + Clone,
  Marshal: ConfigurationMarshal<Config>
{
  pub fn new(mut marshal: Marshal, max_retries: usize, load_default_if_blank: bool) -> Result<Self, ConfigurationError> {
    let current = marshal.read_with_retry(5);
    match current {
      Ok(c) => Ok(Self { volatile: c, marshal, max_retries }),
      Err(ConfigurationError::BlankError) if load_default_if_blank => {
        let c = Config::default();
        marshal.write_with_retry(&c, max_retries)?;
        Ok(Self { volatile: c, marshal, max_retries })
      },
      Err(e) => Err(e)
    }
  }

  // Will also load default if the config is invalid (such as via an upgrade)
  pub fn new_or_default(mut marshal: Marshal, max_retries: usize) -> Result<Self, ConfigurationError> {
    let current = marshal.read_with_retry(5);
    match current {
      Ok(c) => Ok(Self { volatile: c, marshal, max_retries }),
      Err(ConfigurationError::BusError) => {
        Err(ConfigurationError::BusError)
      },
      Err(e) => {
        let c = Config::default();
        marshal.write_with_retry(&c, max_retries)?;
        Ok(Self { volatile: c, marshal, max_retries })
      },
    }
  }

  pub fn set_retries(&mut self, retries: usize) {
    self.max_retries = retries
  }
}

impl<Config, Marshal> GenericConfigurationProvider<Config> for ConfigurationProvider<Config, Marshal>
where
  Config: Default + Clone,
  Marshal: ConfigurationMarshal<Config>
{
  fn current(&self) -> &Config {
    &self.volatile
  }

  fn current_mut(&mut self) -> &mut Config {
    &mut self.volatile
  }

  fn commit(&mut self, reload: bool) -> Result<(), ConfigurationError> {
    self.marshal.write_with_retry(&self.volatile, self.max_retries)?;

    if reload {
      self.reload()?;
    }

    Ok(())
  }

  fn reload(&mut self) -> Result<(), ConfigurationError> {
    self.volatile = self.marshal.read_with_retry(self.max_retries)?;
    Ok(())
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
  type Error = ConfigurationError;

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

  use binmarshal::{DemarshalOwned, Marshal, MarshalError, rw::{BitView, BitWriter, VecBitWriter}};
  use embedded_hal::blocking::{i2c, delay::DelayMs};
  use grapple_m24c64::M24C64;
  use alloc::vec;

  use crate::{ConfigurationError, ConfigurationMarshal};

  pub struct M24C64ConfigurationMarshal<Config, I2C, Delay> {
    delay: Delay,
    address_offset: usize,
    eeprom: M24C64<I2C>,
    marker: PhantomData<Config>
  }

  #[derive(Debug)]
  pub enum M24C64ConfigurationError<E, SErr> {
    Serialisation(SErr),
    I2C(E),
    BlankEeprom
  }

  impl<E, SErr> Into<ConfigurationError> for M24C64ConfigurationError<E, SErr> {
    fn into(self) -> ConfigurationError {
      match self {
        M24C64ConfigurationError::Serialisation(_) => ConfigurationError::SerialisationError,
        M24C64ConfigurationError::I2C(_) => ConfigurationError::BusError,
        M24C64ConfigurationError::BlankEeprom => ConfigurationError::BlankError,
      }
    }
  }

  impl<Config, I2C, Delay> M24C64ConfigurationMarshal<Config, I2C, Delay> {
    #[allow(unused)]
    pub fn new(eeprom: M24C64<I2C>, address: usize, delay: Delay) -> Self {
      Self { delay, address_offset: address, eeprom, marker: PhantomData }
    }
  }

  impl<'a, I2C, Delay, Config, E> ConfigurationMarshal<Config> for M24C64ConfigurationMarshal<Config, I2C, Delay>
  where
    Config: Marshal<()> + DemarshalOwned + Default + Clone,
    I2C: i2c::Write<u8, Error = E> + i2c::WriteRead<u8, Error = E>,
    Delay: DelayMs<u16>
  {
    type Error = M24C64ConfigurationError<E, MarshalError>;

    fn write(&mut self, config: &Config) -> Result<(), Self::Error> {
      let mut writer = VecBitWriter::new();
      if let Err(e) = config.clone().write(&mut writer, ()) {
        return Err(Self::Error::Serialisation(e));
      }
      self.delay.delay_ms(10u16);
      let bytes = writer.slice();
      self.eeprom.write(self.address_offset, &(bytes.len() as u16).to_le_bytes(), &mut self.delay).map_err(|e| Self::Error::I2C(e))?;
      self.delay.delay_ms(10u16);
      self.eeprom.write(self.address_offset + 0x02, &bytes[..], &mut self.delay).map_err(|e| Self::Error::I2C(e))?;
      self.delay.delay_ms(10u16);
      Ok(())
    }

    fn read(&mut self) -> Result<Config, Self::Error> {
      let mut len_buf = [0u8; 2];
      self.delay.delay_ms(1u16);
      self.eeprom.read(self.address_offset, &mut len_buf[..]).map_err(|e| Self::Error::I2C(e))?;

      if len_buf[0] == 255 && len_buf[1] == 255 {
        return Err(Self::Error::BlankEeprom);
      }

      let mut buf = vec![0u8; u16::from_le_bytes(len_buf) as usize];
      self.eeprom.read(self.address_offset + 0x02, &mut buf[..]).map_err(|e| Self::Error::I2C(e))?;
      match Config::read(&mut BitView::new(&buf), ()) {
        Ok(c) => Ok(c),
        Err(e) => Err(Self::Error::Serialisation(e)),
      }
    }
  }
}
