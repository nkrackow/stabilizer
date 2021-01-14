use dsp::{
    iir_int::{IIRState, IIR},
    reciprocal_pll::TimestampHandler,
    shift_round,
    trig::{atan2, cossin},
    Complex,
};

use std::f64::consts::PI;
use std::vec::Vec;

const ADC_MAX: f64 = 1.;
const ADC_MAX_COUNT: f64 = (1 << 15) as f64;

struct Lockin {
    harmonic: u32,
    phase_offset: u32,
    iir: IIR,
    iir_state: [IIRState; 2],
}

impl Lockin {
    /// Construct a new `Lockin` instance.
    ///
    /// # Args
    /// * `harmonic` - Factor for harmonic demodulation. For example, a value of 1 would demodulate
    /// with the reference frequency whereas a value of 2 would demodulate with the first harmonic
    /// of the reference frequency.
    /// * `phase_offset` - Phase offset of the scaled (see `harmonic`) demodulation signal relative
    /// to the reference signal.
    /// * `iir` - IIR coefficients (see `iir_int::IIR`) used for filtering the demodulated in-phase
    /// and quadrature signals.
    pub fn new(harmonic: u32, phase_offset: u32, iir: IIR) -> Self {
        Lockin {
            harmonic,
            phase_offset,
            iir,
            iir_state: [[0; 5]; 2],
        }
    }

    /// Compute the in-phase and quadrature signals. This is intended to mimic the lock-in
    /// processing routine invoked in main.rs.
    ///
    /// # Args
    /// * `adc_samples` - ADC samples.
    /// * `demodulation_initial_phase` - Phase value of the demodulation signal corresponding to the
    /// first ADC sample.
    /// * `demodulation_frequency` - Demodulation frequency.
    pub fn update(
        &mut self,
        adc_samples: Vec<i16>,
        demodulation_initial_phase: u32,
        demodulation_frequency: u32,
    ) -> Complex<i32> {
        let mut signal = Vec::<Complex<i32>>::new();

        adc_samples.iter().enumerate().for_each(|(i, s)| {
            let sample_phase = self
                .harmonic
                .wrapping_mul(
                    (demodulation_frequency.wrapping_mul(i as u32))
                        .wrapping_add(demodulation_initial_phase),
                )
                .wrapping_add(self.phase_offset);
            let (cos, sin) = cossin(sample_phase as i32);

            signal.push((
                *s as i32 * shift_round(sin, 16),
                *s as i32 * shift_round(cos, 16),
            ));

            signal[i].0 = self.iir.update(&mut self.iir_state[0], signal[i].0);
            signal[i].1 = self.iir.update(&mut self.iir_state[1], signal[i].1);
        });

        (signal[0].0, signal[0].1)
    }
}

/// Single-frequency sinusoid.
#[derive(Copy, Clone)]
struct PureSine {
    // Frequency (in Hz).
    frequency: f64,
    // Amplitude in dBFS (decibels relative to full-scale). A 16-bit ADC has a minimum dBFS for each
    // sample of -90.
    amplitude_dbfs: f64,
    // Phase offset (in radians).
    phase_offset: f64,
}

/// Convert a dBFS voltage ratio to a linear ratio.
///
/// # Args
/// * `dbfs` - dB ratio relative to full scale.
///
/// # Returns
/// Linear value.
fn linear(dbfs: f64) -> f64 {
    let base = 10_f64;
    ADC_MAX * base.powf(dbfs / 20.)
}

/// Convert a linear voltage ratio to a dBFS ratio.
///
/// # Args
/// * `linear` - Linear voltage ratio.
///
/// # Returns
/// dBFS value.
fn dbfs(linear: f64) -> f64 {
    20. * (linear / ADC_MAX).log10()
}

/// Convert a real ADC input value in the range `-ADC_MAX` to `+ADC_MAX` to an equivalent 16-bit ADC
/// sampled value. This models the ideal ADC transfer function.
///
/// # Args
/// * `x` - Real ADC input value.
///
/// # Returns
/// Sampled ADC value.
fn real_to_adc_sample(x: f64) -> i16 {
    let max: i32 = i16::MAX as i32;
    let min: i32 = i16::MIN as i32;

    let xi: i32 = (x / ADC_MAX * ADC_MAX_COUNT) as i32;

    // It's difficult to characterize the correct output result when the inputs are clipped, so
    // panic instead.
    if xi > max {
        panic!("Input clipped to maximum, result is unlikely to be correct.");
    } else if xi < min {
        panic!("Input clipped to minimum, result is unlikely to be correct.");
    }

    xi as i16
}

/// Generate a full batch of ADC samples starting at `timestamp_start`.
///
/// # Args
/// * `pure_signals` - Pure sinusoidal components of the ADC-sampled signal.
/// * `timestamp_start` - Starting time of ADC-sampled signal in terms of the internal clock count.
/// * `internal_frequency` - Internal clock frequency (in Hz).
/// * `adc_frequency` - ADC sampling frequency (in Hz).
/// * `sample_buffer_size` - The number of ADC samples in one processing batch.
///
/// # Returns
/// The sampled signal at the ADC input.
fn adc_sampled_signal(
    pure_signals: &Vec<PureSine>,
    timestamp_start: u64,
    internal_frequency: f64,
    adc_frequency: f64,
    sample_buffer_size: u32,
) -> Vec<i16> {
    // amplitude of each pure signal
    let mut amplitude: Vec<f64> = Vec::<f64>::new();
    // initial phase value for each pure signal
    let mut initial_phase: Vec<f64> = Vec::<f64>::new();
    // phase increment at each ADC sample for each pure signal
    let mut phase_increment: Vec<f64> = Vec::<f64>::new();
    let adc_period = internal_frequency / adc_frequency;

    // For each pure sinusoid, compute the amplitude, phase corresponding to the first ADC sample,
    // and phase increment for each subsequent ADC sample.
    for pure_signal in pure_signals.iter() {
        let signal_period = internal_frequency / pure_signal.frequency;
        let phase_offset_count =
            pure_signal.phase_offset / (2. * PI) * signal_period;
        let initial_phase_count =
            (phase_offset_count + timestamp_start as f64) % signal_period;

        amplitude.push(linear(pure_signal.amplitude_dbfs));
        initial_phase.push(2. * PI * initial_phase_count / signal_period);
        phase_increment.push(2. * PI * adc_period / signal_period);
    }

    // Compute the input signal corresponding to each ADC sample by summing the contributions from
    // each pure sinusoid.
    let mut signal = Vec::<i16>::new();

    for i in 0..sample_buffer_size {
        signal.push(real_to_adc_sample(
            amplitude
                .iter()
                .zip(initial_phase.iter())
                .zip(phase_increment.iter())
                .fold(0., |acc, ((a, phi), theta)| {
                    acc + a * (phi + theta * i as f64).sin()
                }),
        ));
    }

    signal
}

/// Reference clock timestamp values in one ADC batch period starting at `timestamp_start`. The
/// number of timestamps in a batch can be 0 or 1, so this returns an Option containing a timestamp
/// only if one occurred during the batch.
///
/// # Args
/// * `reference_frequency` - External reference signal frequency (in Hz).
/// * `timestamp_start` - Start time in terms of the internal clock count. This is the start time of
/// the current processing sequence.
/// * `timestamp_stop` - Stop time in terms of the internal clock count.
/// * `internal_frequency` - Internal clock frequency (in Hz).
///
/// # Returns
/// An Option, containing a timestamp if one occurred during the current batch period.
fn adc_batch_timestamps(
    reference_frequency: f64,
    timestamp_start: u64,
    timestamp_stop: u64,
    internal_frequency: f64,
) -> Option<u32> {
    let reference_period = internal_frequency / reference_frequency;
    let start_count = timestamp_start as f64 % reference_period;

    let timestamp = (reference_period - start_count) % reference_period;

    if timestamp < (timestamp_stop - timestamp_start) as f64 {
        return Some(
            ((timestamp_start + timestamp.round() as u64) % (1u64 << 32))
                as u32,
        );
    }

    None
}

/// Lowpass biquad filter using cutoff and sampling frequencies.  Taken from:
/// https://webaudio.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html
///
/// # Args
/// * `corner_frequency` - Corner frequency, or 3dB cutoff frequency (in Hz).
/// * `sampling_frequency` - Sampling frequency (in Hz).
///
/// # Returns
/// 2nd-order IIR filter coefficients in the form [b0,b1,b2,a1,a2]. a0 is set to -1.
fn lowpass_iir_coefficients(
    corner_frequency: f64,
    sampling_frequency: f64,
) -> [i32; 5] {
    let normalized_angular_frequency: f64 =
        2. * PI * corner_frequency / sampling_frequency;
    let quality_factor: f64 = 1. / 2f64.sqrt();
    let alpha: f64 = normalized_angular_frequency.sin() / (2. * quality_factor);
    // All b coefficients have been multiplied by a factor of 2 in comparison with the link above in
    // order to set the passband gain to 2.
    let mut b0: f64 = 1. - normalized_angular_frequency.cos();
    let mut b1: f64 = 2. * (1. - normalized_angular_frequency.cos());
    let mut b2: f64 = b0;
    let a0: f64 = 1. + alpha;
    let mut a1: f64 = -2. * normalized_angular_frequency.cos();
    let mut a2: f64 = 1. - alpha;
    b0 /= a0;
    b1 /= a0;
    b2 /= a0;
    a1 /= -a0;
    a2 /= -a0;

    // iir uses Q2.30 fixed point
    [
        (b0 * (1 << 30) as f64).round() as i32,
        (b1 * (1 << 30) as f64).round() as i32,
        (b2 * (1 << 30) as f64).round() as i32,
        (a1 * (1 << 30) as f64).round() as i32,
        (a2 * (1 << 30) as f64).round() as i32,
    ]
}

/// Maximum acceptable error between a computed and actual value given fixed and relative
/// tolerances.
///
/// # Args
/// * `a` - First input.
/// * `b` - Second input. The relative tolerance is computed with respect to the maximum of the
/// absolute values of the first and second inputs.
/// * `rtol` - Relative tolerance.
/// * `atol` - Fixed tolerance.
///
/// # Returns
/// Maximum acceptable error.
fn max_error(a: f64, b: f64, rtol: f64, atol: f64) -> f64 {
    rtol * a.abs().max(b.abs()) + atol
}

// TODO this is (mostly) copied from testing.rs.
pub fn isclose(a: f64, b: f64, rtol: f64, atol: f64) -> bool {
    (a - b).abs() <= max_error(a, b, rtol, atol)
}

/// Total noise amplitude of the input signal after sampling by the ADC. This computes an upper
/// bound of the total noise amplitude, rather than its actual value.
///
/// # Args
/// * `noise_inputs` - Noise sources at the ADC input.
/// * `demodulation_frequency` - Frequency of the demodulation signal (in Hz).
/// * `corner_frequency` - Low-pass filter 3dB corner (cutoff) frequency.
///
/// # Returns
/// Upper bound of the total amplitude of all noise sources.
fn sampled_noise_amplitude(
    noise_inputs: &Vec<PureSine>,
    demodulation_frequency: f64,
    corner_frequency: f64,
) -> f64 {
    // There is not a simple way to compute the amplitude of a superpostition of sinusoids with
    // different frequencies and phases. Although we can compute the amplitude in special cases
    // (e.g., two signals whose periods have a common multiple), these do not help us in the general
    // case. However, we can say that the total amplitude will not be greater than the sum of the
    // amplitudes of the individual noise sources. We treat this as an upper bound, and use it as an
    // approximation of the actual amplitude.

    let mut noise: f64 = noise_inputs
        .iter()
        .map(|n| {
            // Noise inputs create an oscillation at the output, where the oscillation magnitude is
            // determined by the strength of the noise and its attenuation (attenuation is
            // determined by its proximity to the demodulation frequency and filter rolloff).
            let octaves = ((n.frequency - demodulation_frequency).abs()
                / corner_frequency)
                .log2();
            // 2nd-order filter. Approximately 12dB/octave rolloff.
            let attenuation = -2. * 20. * 2_f64.log10() * octaves;
            linear(n.amplitude_dbfs + attenuation)
        })
        .sum();

    // Add in 1/2 LSB for the maximum amplitude deviation resulting from quantization.
    noise += 1. / ADC_MAX_COUNT / 2.;

    noise
}

/// Compute the maximum effect of input noise on the lock-in magnitude computation.
///
/// The maximum effect of noise on the magnitude computation is given by:
///
/// | sqrt((I+n*sin(x))**2 + (Q+n*cos(x))**2) - sqrt(I**2 + Q**2) |
///
/// * I is the in-phase component of the portion of the input signal with the same frequency as the
/// demodulation signal.
/// * Q is the quadrature component.
/// * n is the total noise amplitude (from all contributions, after attenuation from filtering).
/// * x is the phase of the demodulation signal.
///
/// We need to find the demodulation phase (x) that maximizes this expression. We can ignore the
/// absolute value operation by also considering the expression minimum. The locations of the
/// minimum and maximum can be computed analytically by finding the value of x when the derivative
/// of this expression with respect to x is 0. When we solve this equation, we find:
///
/// x = atan(I/Q)
///
/// It's worth noting that this solution is technically only valid when cos(x)!=0 (i.e.,
/// x!=pi/2,-pi/2). However, this is not a problem because we only get these values when Q=0. Rust
/// correctly computes atan(inf)=pi/2, which is precisely what we want because x=pi/2 maximizes
/// sin(x) and therefore also the noise effect.
///
/// The other maximum or minimum is pi radians away from this value.
///
/// # Args
/// * `total_noise_amplitude` - Combined amplitude of all noise sources sampled by the ADC.
/// * `in_phase_actual` - Value of the in-phase component if no noise were present at the ADC input.
/// * `quadrature_actual` - Value of the quadrature component if no noise were present at the ADC
/// input.
/// * `desired_input_amplitude` - Amplitude of the desired input signal. That is, the input signal
/// component with the same frequency as the demodulation signal.
///
/// # Returns
/// Approximation of the maximum effect on the magnitude computation due to noise sources at the ADC
/// input.
fn magnitude_noise(
    total_noise_amplitude: f64,
    in_phase_actual: f64,
    quadrature_actual: f64,
    desired_input_amplitude: f64,
) -> f64 {
    // See function documentation for explanation.
    let noise = |in_phase_delta: f64, quadrature_delta: f64| -> f64 {
        (((in_phase_actual + in_phase_delta).powf(2.)
            + (quadrature_actual + quadrature_delta).powf(2.))
        .sqrt()
            - desired_input_amplitude)
            .abs()
    };

    let phase = (in_phase_actual / quadrature_actual).atan();
    let max_noise_1 = noise(
        total_noise_amplitude * phase.sin(),
        total_noise_amplitude * phase.cos(),
    );
    let max_noise_2 = noise(
        total_noise_amplitude * (phase + PI).sin(),
        total_noise_amplitude * (phase + PI).cos(),
    );

    max_noise_1.max(max_noise_2)
}

/// Compute the maximum phase deviation from the correct value due to the input noise sources.
///
/// The maximum effect of noise on the phase computation is given by:
///
/// | atan2(Q+n*cos(x), I+n*sin(x)) - atan2(Q, I) |
///
/// See `magnitude_noise` for an explanation of the terms in this mathematical expression.
///
/// This expression is harder to compute analytically than the expression in `magnitude_noise`. We
/// could compute it numerically, but that's expensive. However, we can use heuristics to try to
/// guess the values of x that will maximize the noise effect. Intuitively, the difference will be
/// largest when the Y-argument of the atan2 function (Q+n*cos(x)) is pushed in the opposite
/// direction of the noise effect on the X-argument (i.e., cos(x) and sin(x) have different
/// signs). We can use:
///
/// * sin(x)=+-1 (+- denotes plus or minus), cos(x)=0,
/// * sin(x)=0, cos(x)=+-1, and
/// * the value of x that maximizes |sin(x)-cos(x)| (when sin(x)=1/sqrt(2) and cos(x)=-1/sqrt(2), or
/// when the signs are flipped)
///
/// The first choice addresses cases in which |I|>>|Q|, the second choice addresses cases in which
/// |Q|>>|I|, and the third choice addresses cases in which |I|~|Q|. We can test all of these cases
/// as an approximation for the real maximum.
///
/// # Args
/// * `total_noise_amplitude` - Total amplitude of all input noise sources.
/// * `in_phase_actual` - Value of the in-phase component if no noise were present at the input.
/// * `quadrature_actual` - Value of the quadrature component if no noise were present at the input.
///
/// # Returns
/// Approximation of the maximum effect on the phase computation due to noise sources at the ADC
/// input.
fn phase_noise(
    total_noise_amplitude: f64,
    in_phase_actual: f64,
    quadrature_actual: f64,
) -> f64 {
    // See function documentation for explanation.
    let noise = |in_phase_delta: f64, quadrature_delta: f64| -> f64 {
        ((quadrature_actual + quadrature_delta)
            .atan2(in_phase_actual + in_phase_delta)
            - quadrature_actual.atan2(in_phase_actual))
        .abs()
    };

    let mut max_noise: f64 = 0.;
    for (in_phase_delta, quadrature_delta) in [
        (
            total_noise_amplitude / 2_f64.sqrt(),
            total_noise_amplitude / -2_f64.sqrt(),
        ),
        (
            total_noise_amplitude / -2_f64.sqrt(),
            total_noise_amplitude / 2_f64.sqrt(),
        ),
        (total_noise_amplitude, 0.),
        (-total_noise_amplitude, 0.),
        (0., total_noise_amplitude),
        (0., -total_noise_amplitude),
    ]
    .iter()
    {
        max_noise = max_noise.max(noise(*in_phase_delta, *quadrature_delta));
    }

    max_noise
}

/// Lowpass filter test for in-phase/quadrature and magnitude/phase computations.
///
/// This attempts to "intelligently" model acceptable tolerance ranges for the measured in-phase,
/// quadrature, magnitude and phase results of lock-in processing for a typical low-pass filter
/// application. So, instead of testing whether the lock-in processing extracts the true magnitude
/// and phase (or in-phase and quadrature components) of the input signal, it attempts to calculate
/// what the lock-in processing should compute given any set of input noise sources. For example, if
/// a noise source of sufficient strength differs in frequency by 1kHz from the reference frequency
/// and the filter cutoff frequency is also 1kHz, testing if the lock-in amplifier extracts the
/// amplitude and phase of the input signal whose frequency is equal to the demodulation frequency
/// is doomed to failure. Instead, this function tests whether the lock-in correctly adheres to its
/// actual transfer function, whether or not it was given reasonable inputs. The logic for computing
/// acceptable tolerance ranges is performed in `sampled_noise_amplitude`, `magnitude_noise`, and
/// `phase_noise`.
///
/// # Args
/// * `internal_frequency` - Internal clock frequency (Hz). The internal clock increments timestamp
/// counter values used to record the edges of the external reference.
/// * `adc_frequency` - ADC sampling frequency (in Hz).
/// * `reference_frequency` - External reference frequency (in Hz).
/// * `demodulation_phase_offset` - Phase offset applied to the in-phase and quadrature demodulation
/// signals.
/// * `harmonic` - Scaling factor for the demodulation frequency. E.g., 2 would demodulate with the
/// first harmonic of the reference frequency.
/// * `sample_buffer_size_log2` - The base-2 logarithm of the number of samples in a processing
/// batch.
/// * `pll_shift_frequency` - See `pll::update()`.
/// * `pll_shift_phase` - See `pll::update()`.
/// * `corner_frequency` - Lowpass filter 3dB cutoff frequency.
/// * `desired_input` - `PureSine` giving the frequency, amplitude and phase of the desired result.
/// * `noise_inputs` - Vector of `PureSine` for any noise inputs on top of `desired_input`.
/// * `time_constant_factor` - Number of time constants after which the output is considered valid.
/// * `tolerance` - Acceptable relative tolerance for the magnitude and angle outputs. This is added
/// to fixed tolerance values computed inside this function. The outputs must remain within this
/// tolerance between `time_constant_factor` and `time_constant_factor+1` time constants.
fn lowpass_test(
    internal_frequency: f64,
    adc_frequency: f64,
    reference_frequency: f64,
    demodulation_phase_offset: f64,
    harmonic: u32,
    sample_buffer_size_log2: usize,
    pll_shift_frequency: u8,
    pll_shift_phase: u8,
    corner_frequency: f64,
    desired_input: PureSine,
    noise_inputs: &mut Vec<PureSine>,
    time_constant_factor: f64,
    tolerance: f64,
) {
    assert!(
        isclose((internal_frequency / adc_frequency).log2(), (internal_frequency / adc_frequency).log2().round(), 0., 1e-5),
        "The number of internal clock cycles in one ADC sampling period must be a power-of-two."
    );

    assert!(
        internal_frequency / reference_frequency
            >= internal_frequency / adc_frequency
            * (1 << sample_buffer_size_log2) as f64,
        "Too many timestamps per batch. Each batch can have at most 1 timestamp."
    );

    let adc_sample_ticks_log2 =
        (internal_frequency / adc_frequency).log2().round() as usize;
    assert!(
        adc_sample_ticks_log2 + sample_buffer_size_log2 <= 32,
        "The base-2 log of the number of ADC ticks in a sampling period plus the base-2 log of the sample buffer size must be less than 32."
    );

    let mut lockin = Lockin::new(
        harmonic,
        (demodulation_phase_offset / (2. * PI) * (1_u64 << 32) as f64).round()
            as u32,
        IIR {
            ba: lowpass_iir_coefficients(corner_frequency, adc_frequency),
        },
    );
    let mut timestamp_handler = TimestampHandler::new(
        pll_shift_frequency,
        pll_shift_phase,
        adc_sample_ticks_log2,
        sample_buffer_size_log2,
    );

    let mut timestamp_start: u64 = 0;
    let time_constant: f64 = 1. / (2. * PI * corner_frequency);
    // Account for the pll settling time (see its documentation).
    let pll_time_constant_samples =
        (1 << pll_shift_phase.max(pll_shift_frequency)) as usize;
    let low_pass_time_constant_samples =
        (time_constant_factor * time_constant * adc_frequency
            / (1 << sample_buffer_size_log2) as f64) as usize;
    let samples = pll_time_constant_samples + low_pass_time_constant_samples;
    // Ensure the result remains within tolerance for 1 time constant after `time_constant_factor`
    // time constants.
    let extra_samples = (time_constant * adc_frequency) as usize;
    let batch_sample_count =
        1_u64 << (adc_sample_ticks_log2 + sample_buffer_size_log2);

    let effective_phase_offset =
        desired_input.phase_offset - demodulation_phase_offset;
    let in_phase_actual =
        linear(desired_input.amplitude_dbfs) * effective_phase_offset.cos();
    let quadrature_actual =
        linear(desired_input.amplitude_dbfs) * effective_phase_offset.sin();

    let total_noise_amplitude = sampled_noise_amplitude(
        noise_inputs,
        reference_frequency * harmonic as f64,
        corner_frequency,
    );
    // Add some fixed error to account for errors introduced by the PLL, our custom trig functions
    // and integer division. It's a bit difficult to be precise about this. I've added a 1%
    // (relative to full scale) error.
    let total_magnitude_noise = magnitude_noise(
        total_noise_amplitude,
        in_phase_actual,
        quadrature_actual,
        linear(desired_input.amplitude_dbfs),
    ) + 1e-2;
    let total_phase_noise =
        phase_noise(total_noise_amplitude, in_phase_actual, quadrature_actual)
            + 1e-2 * 2. * PI;

    let pure_signals = noise_inputs;
    pure_signals.push(desired_input);

    for n in 0..(samples + extra_samples) {
        let adc_signal = adc_sampled_signal(
            &pure_signals,
            timestamp_start,
            internal_frequency,
            adc_frequency,
            1 << sample_buffer_size_log2,
        );
        let timestamp = adc_batch_timestamps(
            reference_frequency,
            timestamp_start,
            timestamp_start + batch_sample_count - 1,
            internal_frequency,
        );

        let (demodulation_initial_phase, demodulation_frequency) =
            timestamp_handler.update(timestamp);

        let (in_phase, quadrature) = lockin.update(
            adc_signal,
            demodulation_initial_phase,
            demodulation_frequency,
        );

        let magnitude = shift_round(in_phase, 16) * shift_round(in_phase, 16)
            + shift_round(quadrature, 16) * shift_round(quadrature, 16);
        let phase = atan2(quadrature, in_phase);

        // Ensure stable within tolerance for 1 time constant after `time_constant_factor`.
        if n >= samples {
            // We want our full-scale magnitude to be 1. Our fixed-point numbers treated as integers
            // set the full-scale magnitude to 1<<60. So, we must divide by this number. However,
            // we've already divided by 1<<32 in the magnitude computation to keep our values within
            // the i32 limits, so we just need to divide by an additional 1<<28.
            let amplitude_normalized =
                (magnitude as f64 / (1_u64 << 28) as f64).sqrt();
            assert!(
                isclose(linear(desired_input.amplitude_dbfs), amplitude_normalized, tolerance, total_magnitude_noise),
                "magnitude actual: {:.4} ({:.2} dBFS), magnitude computed: {:.4} ({:.2} dBFS), tolerance: {:.4}",
                linear(desired_input.amplitude_dbfs),
                desired_input.amplitude_dbfs,
                amplitude_normalized,
                dbfs(amplitude_normalized),
                max_error(linear(desired_input.amplitude_dbfs), amplitude_normalized, tolerance, total_magnitude_noise),
            );
            let phase_normalized =
                phase as f64 / (1_u64 << 32) as f64 * (2. * PI);
            assert!(
                isclose(
                    effective_phase_offset,
                    phase_normalized,
                    tolerance,
                    total_phase_noise
                ),
                "phase actual: {:.4}, phase computed: {:.4}, tolerance: {:.4}",
                effective_phase_offset,
                phase_normalized,
                max_error(
                    effective_phase_offset,
                    phase_normalized,
                    tolerance,
                    total_phase_noise
                ),
            );

            let in_phase_normalized = in_phase as f64 / (1 << 30) as f64;
            let quadrature_normalized = quadrature as f64 / (1 << 30) as f64;

            assert!(
                isclose(
                    in_phase_actual,
                    in_phase_normalized,
                    total_noise_amplitude,
                    tolerance
                ),
                "in-phase actual: {:.4}, in-phase computed: {:.3}, tolerance: {:.4}",
                in_phase_actual,
                in_phase_normalized,
                max_error(
                    in_phase_actual,
                    in_phase_normalized,
                    total_noise_amplitude,
                    tolerance
                ),
            );
            assert!(
                isclose(
                    quadrature_actual,
                    quadrature_normalized,
                    total_noise_amplitude,
                    tolerance
                ),
                "quadrature actual: {:.4}, quadrature computed: {:.4}, tolerance: {:.4}",
                quadrature_actual,
                quadrature_normalized,
                max_error(
                    quadrature_actual,
                    quadrature_normalized,
                    total_noise_amplitude,
                    tolerance
                ),
            );
        }

        timestamp_start += batch_sample_count;
    }
}

#[test]
fn lowpass() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 100e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 3;
    let pll_shift_phase: u8 = 2;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 6.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.1 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.9 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_demodulation_phase_offset_pi_2() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 100e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 3;
    let pll_shift_phase: u8 = 2;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = PI / 2.;
    let time_constant_factor: f64 = 6.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.1 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.9 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_phase_offset_pi_2() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 100e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 3;
    let pll_shift_phase: u8 = 2;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 6.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: PI / 2.,
        },
        &mut vec![
            PureSine {
                frequency: 1.1 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.9 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_fundamental_111e3_phase_offset_pi_4() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 111e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 3;
    let pll_shift_phase: u8 = 2;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: PI / 4.,
        },
        &mut vec![
            PureSine {
                frequency: 1.1 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.9 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_first_harmonic() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 50e3;
    let harmonic: u32 = 2;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.2 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.8 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_second_harmonic() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 50e3;
    let harmonic: u32 = 3;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.2 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.8 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_third_harmonic() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 50e3;
    let harmonic: u32 = 4;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.2 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.8 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_first_harmonic_phase_shift() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 50e3;
    let harmonic: u32 = 2;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: PI / 4.,
        },
        &mut vec![
            PureSine {
                frequency: 1.2 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.8 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_adc_frequency_1e6() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 32.;
    let signal_frequency: f64 = 100e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.2 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.8 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_internal_frequency_125e6() {
    let internal_frequency: f64 = 125e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 100e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-2;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![
            PureSine {
                frequency: 1.2 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
            PureSine {
                frequency: 0.8 * demodulation_frequency,
                amplitude_dbfs: -20.,
                phase_offset: 0.,
            },
        ],
        time_constant_factor,
        tolerance,
    );
}

#[test]
fn lowpass_low_signal_frequency() {
    let internal_frequency: f64 = 100e6;
    let adc_frequency: f64 = internal_frequency / 64.;
    let signal_frequency: f64 = 10e3;
    let harmonic: u32 = 1;
    let sample_buffer_size_log2: usize = 2;
    let pll_shift_frequency: u8 = 2;
    let pll_shift_phase: u8 = 1;
    let corner_frequency: f64 = 1e3;
    let demodulation_frequency: f64 = harmonic as f64 * signal_frequency;
    let demodulation_phase_offset: f64 = 0.;
    let time_constant_factor: f64 = 5.;
    let tolerance: f64 = 1e-1;

    lowpass_test(
        internal_frequency,
        adc_frequency,
        signal_frequency,
        demodulation_phase_offset,
        harmonic,
        sample_buffer_size_log2,
        pll_shift_frequency,
        pll_shift_phase,
        corner_frequency,
        PureSine {
            frequency: demodulation_frequency,
            amplitude_dbfs: -30.,
            phase_offset: 0.,
        },
        &mut vec![PureSine {
            frequency: 1.1 * demodulation_frequency,
            amplitude_dbfs: -20.,
            phase_offset: 0.,
        }],
        time_constant_factor,
        tolerance,
    );
}
