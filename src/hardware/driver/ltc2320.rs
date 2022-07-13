///! LTC2320 Driver
///!
///! QSPI bug (2.4.3):
///! https://www.st.com/resource/en/errata_sheet/es0392-stm32h742xig-and-stm32h743xig-device-limitations-stmicroelectronics.pdf
///!
///! This driver is intended to be used in the following manner:
///! 1. Trigger a new LTC2320 conversion with start_conversion(). This sets nCNV low and starts
///!    a hardware timer to trigger an interrupt when TCONV has passed.
///! 2. Call handle_conv_done_irq() in the timer ISR to stop and reset the timer and start the
///!    QSPI readout. The QSPI peripheral will trigger another interrupt once that transfer is done.
///! 3. Call handle_transfer_done_irq() in the QSPI ISR to retrieve the ADC data and set nCNV high again.
///!
///! Only works under the following condition:
///! Conversions are not restarted faster than (T_readout + TCONV + TCNVH + readout/irq CPU overhead).
use super::super::hal::{
    device::QUADSPI,
    gpio::{self, gpiob, gpioc, gpioe},
    prelude::*,
    rcc, stm32,
    time::NanoSeconds,
    timer::{self, Timer},
    xspi::{Qspi, QspiError, QspiMode, XspiExt},
};
use core::ptr;
use fugit::Hertz;

#[derive(Copy, Clone, Debug)]
pub struct TimerRunningError;

pub struct Ltc2320Pins {
    pub spi: (
        gpiob::PB2<gpio::Alternate<9>>,
        gpioe::PE7<gpio::Alternate<10>>,
        gpioe::PE8<gpio::Alternate<10>>,
        gpioe::PE9<gpio::Alternate<10>>,
        gpioe::PE10<gpio::Alternate<10>>,
    ),
    pub cnv: gpioc::PC11<gpio::Output<gpio::PushPull>>,
}

pub struct Ltc2320 {
    qspi: Qspi<QUADSPI>,
    cnv: gpioc::PC11<gpio::Output<gpio::PushPull>>,
    timer: Timer<stm32::TIM7>,
}

impl Ltc2320 {
    const N_BYTES: usize = 16; // Number of bytes to be transfered.
    const TCONV: NanoSeconds = NanoSeconds::from_ticks(450u32); // minimum conversion time according to datasheet
    pub fn new(
        clocks: &rcc::CoreClocks,
        qspi_rec: rcc::rec::Qspi,
        qspi_peripheral: stm32::QUADSPI,
        qspi_frequency: Hertz<u32>,
        timer_rec: rcc::rec::Tim7,
        timer_peripheral: stm32::TIM7,
        timer_frequency: Hertz<u32>,
        mut pins: Ltc2320Pins,
    ) -> Self {
        let mut qspi =
            qspi_peripheral.bank2(pins.spi, qspi_frequency, clocks, qspi_rec);
        qspi.configure_mode(QspiMode::OneBit).unwrap();
        qspi.is_busy().unwrap(); // panic if qspi busy
        qspi.inner_mut().ccr.modify(|_, w| unsafe {
            w.dcyc()
                .bits(0) // set nr dummy cycles to 0 (disable dummy phase)
                .abmode()
                .bits(0) // disable alternate-bytes phase
                .admode()
                .bits(1) // enable address phase
                .adsize()
                .bits(0b01) // set to 16 bit address to gain readback data alignment
                .imode()
                .bits(0) // disable instruction phase
                .fmode()
                .bits(0b01) // indirect read mode
        });
        qspi.inner_mut().cr.modify(
            |_, w| w.tcie().bit(true), // enable transfer complete interrupt
        );
        pins.cnv.set_high();
        // Setup timer with dummy timeout. Timer tick frequency updated below.
        let mut timer = timer_peripheral.timer(1.kHz(), timer_rec, clocks);
        timer.pause();
        timer.set_tick_freq(timer_frequency);
        timer.listen(timer::Event::TimeOut);
        Self {
            qspi,
            cnv: pins.cnv,
            timer,
        }
    }

    /// et nCNV low and setup timer to wait for 450 ns.
    /// Note that the CPU overhead for handling the irq leads to additional delay.
    pub fn start_conversion(&mut self) -> Result<(), TimerRunningError> {
        self.cnv.set_low();
        // check if the timer is running
        let cr1 = self.timer.inner().cr1.read();
        if cr1.cen().bit() == true {
            return Err(TimerRunningError);
        }
        self.timer.start(Ltc2320::TCONV.into_rate());
        Ok(())
    }

    /// Clear timer interrupt start QSPI read of ADC data.
    pub fn handle_conv_done_irq(&mut self) -> Result<(), QspiError> {
        self.timer.pause();
        self.timer.reset_counter();
        self.timer.clear_irq();
        // zero dummy address due to QSPI silicon bug
        self.qspi.begin_read(0, Ltc2320::N_BYTES)
    }

    /// Set nCNV high, readout QSPI buffer, bitshuffle.
    pub fn handle_transfer_done_irq(&mut self, data: &mut [u16]) {
        self.cnv.set_high(); // TCNVH: has to be high for at least 30 ns (8 cycles)
        self.qspi.inner_mut().fcr.modify(
            |_, w| w.ctcf().bit(true), // clear transfer complete flag
        );
        // Read data from the FIFO.
        unsafe {
            for location in data {
                *location = ptr::read_volatile(
                    &self.qspi.inner().dr as *const _ as *const u16,
                );
            }
        }
    }
}
