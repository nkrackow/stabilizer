///! Driver relays driver
///!
///! There are 2 relays at the driver output:
///!    - one that shorts the output to ground
///!    - one that connects the current source/sink to the output
///!
///! The relays are controlled via an I2C io-expander.
use super::hal::rcc;
/// Todo: document relay toggeling
use core::fmt::Debug;
use embedded_hal::blocking::{
    delay::DelayUs,
    i2c::{Write, WriteRead},
};
use mcp230xx::{Level, Mcp23008, Mcp230xx};

use super::Channel;
use smlang::statemachine;

#[derive(Debug, Copy, Clone)]
pub enum RelayError {
    /// Indicates that the I2C expander IC is in use
    Mcp23008InUse,
}

// Driver low noise output relays pins
#[allow(non_camel_case_types)]
#[derive(Debug, Copy, Clone, PartialEq)]
enum RelayPin {
    LN_K1_EN_N,
    LN_K1_EN,
    LN_K0_D,
    LN_K0_CP,
    HP_K1_EN_N,
    HP_K1_EN,
    HP_K0_D,
    HP_K0_CP,
}

impl From<RelayPin> for Mcp23008 {
    fn from(pin: RelayPin) -> Self {
        match pin {
            RelayPin::LN_K1_EN_N => Self::P0,
            RelayPin::LN_K1_EN => Self::P1,
            RelayPin::LN_K0_D => Self::P2,
            RelayPin::LN_K0_CP => Self::P3,
            RelayPin::HP_K1_EN_N => Self::P4,
            RelayPin::HP_K1_EN => Self::P5,
            RelayPin::HP_K0_D => Self::P6,
            RelayPin::HP_K0_CP => Self::P7,
        }
    }
}

// small helper to lock the mutex
// maybe todo: Pass out an error if in use. Not sure how to get that out of the state machine though..
fn get_mcp<I2C>(
    mutex: &'_ spin::Mutex<Mcp230xx<I2C, Mcp23008>>,
) -> spin::MutexGuard<Mcp230xx<I2C, Mcp23008>> {
    mutex.try_lock().unwrap() // panic here if in use
}

pub struct Relay<'a, I2C: WriteRead + Write> {
    mutex: &'a spin::Mutex<Mcp230xx<I2C, Mcp23008>>,
    delay: asm_delay::AsmDelay,
    k1_en_n: RelayPin,
    k1_en: RelayPin,
    k0_d: RelayPin,
    k0_cp: RelayPin,
}

impl<'a, I2C, E> Relay<'a, I2C>
where
    I2C: WriteRead<Error = E> + Write<Error = E>,
{
    const K0_DELAY: fugit::MillisDuration<u64> =
        fugit::MillisDurationU64::millis(10);
    const K1_DELAY: fugit::MillisDuration<u64> =
        fugit::MillisDurationU64::millis(10);
    pub fn new(
        mutex: &'a spin::Mutex<Mcp230xx<I2C, Mcp23008>>,
        ccdr: &rcc::CoreClocks,
        ch: Channel,
    ) -> Self {
        let (k1_en_n, k1_en, k0_d, k0_cp) = if ch == Channel::LowNoise {
            (
                RelayPin::LN_K1_EN_N,
                RelayPin::LN_K1_EN,
                RelayPin::LN_K0_D,
                RelayPin::LN_K0_CP,
            )
        } else {
            (
                RelayPin::HP_K1_EN_N,
                RelayPin::HP_K1_EN,
                RelayPin::HP_K0_D,
                RelayPin::HP_K0_CP,
            )
        };
        let delay = asm_delay::AsmDelay::new(asm_delay::bitrate::Hertz(
            ccdr.c_ck().to_Hz(),
        ));
        Relay {
            mutex,
            delay,
            k1_en_n,
            k1_en,
            k0_d,
            k0_cp,
        }
    }
}

pub mod sm {
    use super::*;
    statemachine! {
        transitions: {
            *Disabled + Enable / engage_k0 = EnableWaitK0,
            EnableWaitK0 + RelayDone / disengage_k1 = EnableWaitK1,
            EnableWaitK1 + RelayDone = Enabled,
            Enabled + Disable / engage_k1 = DisableWaitK1,
            DisableWaitK1 + RelayDone / disengage_k0 = DisableWaitK0,
            DisableWaitK0 + RelayDone = Disabled
        }
    }
}

impl<'a, I2C, E> sm::StateMachineContext for Relay<'a, I2C>
where
    I2C: WriteRead<Error = E> + Write<Error = E>,
    E: Debug,
{
    // set K0 to upper position
    fn engage_k0(&mut self) {
        let mut mcp = get_mcp(self.mutex);
        // set flipflop data pin
        mcp.set_gpio(self.k0_d.into(), Level::High).unwrap();
        // set flipflop clock input low to prepare rising edge
        mcp.set_gpio(self.k0_cp.into(), Level::Low).unwrap();
        self.delay.delay_us(100u32);
        // set flipflop clock input high to generate rising edge
        mcp.set_gpio(self.k0_cp.into(), Level::High).unwrap();
    }

    // set K0 to lower position
    fn disengage_k0(&mut self) {
        let mut mcp = get_mcp(self.mutex);
        mcp.set_gpio(self.k0_d.into(), Level::High).unwrap();
        mcp.set_gpio(self.k0_cp.into(), Level::Low).unwrap();
        self.delay.delay_us(100u32);
        mcp.set_gpio(self.k0_cp.into(), Level::High).unwrap();
    }

    // set K1 to upper position
    fn disengage_k1(&mut self) {
        let mut mcp = get_mcp(self.mutex);
        // set en high and en _n low in order to engage K1
        mcp.set_gpio(self.k1_en.into(), Level::Low).unwrap();
        mcp.set_gpio(self.k1_en_n.into(), Level::High).unwrap();
    }

    // set K1 to lower position
    fn engage_k1(&mut self) {
        let mut mcp = get_mcp(self.mutex);
        mcp.set_gpio(self.k1_en.into(), Level::High).unwrap();
        mcp.set_gpio(self.k1_en_n.into(), Level::Low).unwrap();
    }
}

impl<'a, I2C, E> sm::StateMachine<Relay<'a, I2C>>
where
    I2C: WriteRead<Error = E> + Write<Error = E>,
    E: Debug,
{
    /// Start relay enabling sequence. Returns the relay delay we need to wait for.
    pub fn enable(&mut self) -> fugit::MillisDuration<u64> {
        self.process_event(sm::Events::Enable).unwrap();
        Relay::<'_, I2C>::K0_DELAY // engage K0 first
    }

    /// Start relay disabling sequence. Returns the relay delay we need to wait for.
    pub fn disable(&mut self) -> fugit::MillisDuration<u64> {
        self.process_event(sm::Events::Disable).unwrap();
        Relay::<'_, I2C>::K1_DELAY // engage K1 first
    }

    /// Handle a completed relay transition. Returns `Some(relay delay)` if we need to wait,
    /// otherwise returns `None`.
    pub fn handle_relay(&mut self) -> Option<fugit::MillisDuration<u64>> {
        self.process_event(sm::Events::RelayDone).unwrap();
        match *self.state() {
            sm::States::EnableWaitK1 => Some(Relay::<'_, I2C>::K1_DELAY), // disengage K1 second
            sm::States::DisableWaitK0 => Some(Relay::<'_, I2C>::K0_DELAY), // disengage K0 second
            _ => None, // done, no delay needed
        }
    }
}

/// A SharedMcp can provide ownership of one of two sets of relays on Driver.
/// Each set consists of two relays that control the state of a Driver output channel.
/// The relays are controlled by toggeling pins on an MCP23008 I2C IO expander on the Driver board.
/// Both sets use the same MCP23008 chip, hence the SharedMcp.
pub struct SharedMcp<I2C1> {
    mutex: spin::Mutex<Mcp230xx<I2C1, Mcp23008>>,
}

impl<I2C, E> SharedMcp<I2C>
where
    I2C: WriteRead<Error = E> + Write<Error = E>,
{
    /// Construct a new shared MCP23008.
    ///
    /// # Args
    /// * `mcp` - The MCP23008 peripheral to share.
    pub fn new(mcp: Mcp230xx<I2C, Mcp23008>) -> Self {
        Self {
            mutex: spin::Mutex::new(mcp),
        }
    }

    /// Allocate a set of relay pins on the MCP23008 and get the [Relay]
    ///
    /// # Args
    /// * `ch` - The Driver channel the relay pins are used for.
    ///
    /// # Returns
    /// An instantiated [Relay] whose ownership can be transferred to other drivers.
    pub fn obtain_relay(
        &self,
        ccdr: &rcc::CoreClocks,
        ch: Channel,
    ) -> Relay<'_, I2C> {
        Relay::new(&self.mutex, ccdr, ch)
    }
}
