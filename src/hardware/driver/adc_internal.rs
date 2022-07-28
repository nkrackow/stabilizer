// This is a dummy driver for the Driver analog reads of the output Voltage and Current.
// Exact Sacales and Pinout will be filled in once we have HW.

use super::super::hal::{
    adc,
    gpio::{gpiof::*, Analog},
    hal::blocking::delay::DelayUs,
    prelude::*,
    rcc::{rec, CoreClocks},
    stm32::{ADC1, ADC12_COMMON, ADC2, ADC3, ADC3_COMMON},
};

const V_REF: f32 = 2.048; // ADC reference voltage
const R_SENSE: f32 = 0.1; // Driver output current sense resistor (Will maybe be something else on HW)

pub enum AdcChannel {
    OutputVoltage(OutputChannelIdx),
    OutputCurrent(OutputChannelIdx),
}
use super::OutputChannelIdx;

pub struct AdcInternalPins {
    pub output_voltage: (PF11<Analog>, PF3<Analog>), // JADC1_IN2_P, JADC3_IN5_P
    pub output_current: (PF12<Analog>, PF4<Analog>), // JADC1_IN2_N, JADC3_IN5_N
}

pub struct AdcInternal {
    adc1: adc::Adc<ADC1, adc::Enabled>,
    adc3: adc::Adc<ADC3, adc::Enabled>,
    pins: AdcInternalPins,
}

impl AdcInternal {
    // ADC sampling time (aka time to charge the sampling capacitor).
    // For the RC circuit on the Stabilizer CPU ADC header, sampling times of more than
    // 160 ns were not observed to have an effect on the sampled values.
    // According to the datasheet the slow ADC channels, of which we use some,
    // need at least 172 ns sampling time for 16 bits accuracy.
    // 64 cycles with a 50 MHz ADC clock gives 1.28 us.
    const STABILIZER_T_SAMP: adc::AdcSampleTime = adc::AdcSampleTime::T_64;
    pub fn new(
        delay: &mut impl DelayUs<u8>,
        clocks: &CoreClocks,
        adc_rcc: (rec::Adc12, rec::Adc3),
        adc: (ADC1, ADC2, ADC3, ADC12_COMMON, ADC3_COMMON),
        pins: AdcInternalPins,
    ) -> Self {
        // Setup ADCs
        let (mut adc1, _adc2) =
            adc::adc12(adc.0, adc.1, delay, adc_rcc.0, clocks);
        adc.3.ccr.modify(|_, w| w.presc().div2()); // Set ADC 1/2 clock prescaler after adc init but before enable
        adc1.calibrate(); // re-calibrate after clock has changed
        let mut adc1 = adc1.enable();
        adc1.set_resolution(adc::Resolution::SIXTEENBIT);
        adc1.set_sample_time(AdcInternal::STABILIZER_T_SAMP);

        let mut adc3 = adc::Adc::adc3(adc.2, delay, adc_rcc.1, clocks);
        adc.4.ccr.modify(|_, w| w.presc().div2()); // Set ADC 3 clock prescaler after adc init but before enable
        adc3.calibrate(); // re-calibrate after clock has changed
        let mut adc3 = adc3.enable();
        adc3.set_resolution(adc::Resolution::SIXTEENBIT);
        adc3.set_sample_time(AdcInternal::STABILIZER_T_SAMP);

        AdcInternal { adc1, adc3, pins }
    }

    pub fn read(&mut self, ch: AdcChannel) -> f32 {
        match ch {
            AdcChannel::OutputVoltage(ch) => self.read_output_voltage(ch),
            AdcChannel::OutputCurrent(ch) => self.read_output_current(ch),
        }
    }

    pub fn read_output_voltage(&mut self, ch: OutputChannelIdx) -> f32 {
        let p = &mut self.pins.output_voltage;
        let code: u32 = match ch {
            OutputChannelIdx::Zero => self.adc1.read(&mut p.0),
            OutputChannelIdx::One => self.adc3.read(&mut p.1),
        }
        .unwrap();
        const SCALE: f32 = V_REF; // Differential voltage sense gain      ToDo
        const OFFSET: f32 = 0.0; // Differential voltage sense offset       ToDo
        (code as f32 / self.adc1.slope() as f32 + OFFSET) * SCALE
    }

    pub fn read_output_current(&mut self, ch: OutputChannelIdx) -> f32 {
        let p = &mut self.pins.output_current;
        let code: u32 = match ch {
            OutputChannelIdx::Zero => self.adc1.read(&mut p.0),
            OutputChannelIdx::One => self.adc3.read(&mut p.1),
        }
        .unwrap();
        const SCALE: f32 = V_REF / R_SENSE; // Current sense scale       ToDo
        const OFFSET: f32 = 0.0; // Current sense offset         ToDo
        (code as f32 / self.adc1.slope() as f32 + OFFSET) * SCALE
    }
}
