from stabilizer_if import *

s=stabilizerClass("ms_control")
s.stream_kill=0
s.rewrite_conf=0
s.app_mode='Man'
s.telemetry_period=1
s.gain_afe0='G2'
s.gain_afe1='G1'
s.lut_config0_amplitude=1
s.lut_config0_phase_offset_deg=0
s.lut_config1_amplitude=1.0
s.lut_config1_phase_offset_deg=110
s.ctrl_offset=6.145
s.sig_ctrl_signal='Triangle'
s.sig_ctrl_frequency=7.335956280048077
s.sig_ctrl_amplitude=0.1
s.sig_ctrl_offset=0
s.sig_ctrl_stream_trigger='PeakMin'
s.stream_request_length=500
s.stream_request_unit='frames'
s.set_stream_length(s.stream_request_length, s.stream_request_unit)
s.streams=['ErrMod', 'Mod', 'ErrDemod', 'CtrlDac']
iir_ctrl=s.add_iir("iir_ctrl", s.sampling_freq/s.batch_size)
s.iir_ctrl.Kp=-0.1
s.iir_ctrl.Ki=-100
s.iir_ctrl.Kd=-2e-05
iir_ctrl.ba=s.iir_ctrl.compute_coeff()
#s.iir_ctrl.ba=[0,0,0,0,0]
s.iir_ctrl.y_offset=0
s.iir_ctrl.y_min=-2
s.iir_ctrl.y_max=2
s.lines_config_threshold=0.025
s.lines_config_offset=0
s.lines_config_hysteresis=0.005
plot=s.add_plot()
s.stream_decimation=1
s.plot.plots=['ErrMod', 'Mod', 'ErrDemod', 'CtrlDac']
s.plot.xlim='auto'
#s.plot.xlim=(-1,1)
s.plot.ylim='auto'
#s.plot.ylim=[(-1,1),(-1,1),(-1,1)]
s.plot.xtype='time_ms'
s.plot.tolerance=0.2
s.plot.refresh_ylim=1
