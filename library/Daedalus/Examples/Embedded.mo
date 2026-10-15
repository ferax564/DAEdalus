within Daedalus.Examples;
package Embedded "Controllers for embedded C code generation and processor-in-the-loop tests"
  extends Daedalus.Icons.ExamplesPackage;

  block DiscretePID "Sampled PID speed controller: anti-windup, derivative on the measurement"
    extends Daedalus.Icons.Block;
    parameter Real kp=0.04 "Proportional gain [V/(rad/s)]";
    parameter Real ki=0.4 "Integral gain [V/(rad/s)/s]";
    parameter Real kd=0.0005 "Derivative gain [V/(rad/s^2)]";
    parameter Units.Time Tf=0.02 "Derivative filter time constant";
    parameter Units.Time Ts=0.01 "Sample period";
    parameter Real uMax=12 "Output limit [V]";
    Daedalus.Blocks.Interfaces.RealInput r "Set point"
      annotation (Placement(transformation(extent={{-140,40},{-100,80}})));
    Daedalus.Blocks.Interfaces.RealInput y "Measurement"
      annotation (Placement(transformation(extent={{-140,-80},{-100,-40}})));
    Daedalus.Blocks.Interfaces.RealOutput u "Actuator command"
      annotation (Placement(transformation(extent={{100,-10},{120,10}})));
  protected
    discrete Real e(start=0, fixed=true) "Control error";
    discrete Real I(start=0, fixed=true) "Integral part";
    discrete Real D(start=0, fixed=true) "Filtered derivative part";
    discrete Real v(start=0, fixed=true) "Unlimited output";
    discrete Real yPrev(start=0, fixed=true) "Measurement at the previous sample";
    discrete Real uOut(start=0, fixed=true);
  equation
    when sample(0, Ts) then
      e = r - y;
      D = (Tf*pre(D) - kd*(y - pre(yPrev)))/(Tf + Ts);
      v = kp*e + pre(I) + D;
      uOut = min(max(v, -uMax), uMax);
      // conditional integration: hold the integral while the output saturates in the error's direction
      I = if v > uMax and e > 0 or v < -uMax and e < 0 then pre(I) else pre(I) + ki*Ts*e;
      yPrev = y;
    end when;
    u = uOut;
    annotation (Icon(graphics={Text(extent={{-80,40},{80,-40}}, textString="PID z")}),
      Documentation(info="<html>
<p>A discrete PID written the way it runs on a microcontroller: one update per sample,
backward-Euler filtered derivative of the measurement (no derivative kick on set-point
steps) and conditional integration as anti-windup. Generate C for it with</p>
<pre>daedalus codegen-c -m Daedalus.Examples.Embedded.DiscretePID --step 0.01 -o pid</pre>
</html>"));
  end DiscretePID;

  block SpeedPI "Continuous PI speed controller, discretised by the code generator"
    extends Daedalus.Icons.Block;
    parameter Real k=0.04 "Gain [V/(rad/s)]";
    parameter Units.Time Ti=0.1 "Integral time";
    parameter Real uMax=12 "Output limit [V]";
    Daedalus.Blocks.Interfaces.RealInput r "Set point"
      annotation (Placement(transformation(extent={{-140,40},{-100,80}})));
    Daedalus.Blocks.Interfaces.RealInput y "Measurement"
      annotation (Placement(transformation(extent={{-140,-80},{-100,-40}})));
    Daedalus.Blocks.Interfaces.RealOutput u "Actuator command"
      annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    Daedalus.Blocks.Continuous.PI pi(k=k, Ti=Ti, yMax=uMax);
  equation
    pi.u = r - y;
    u = pi.y;
    annotation (Icon(graphics={Text(extent={{-80,40},{80,-40}}, textString="PI")}));
  end SpeedPI;

  model MotorBench "DC motor with a load step and a speed set-point profile: the plant side of the loop"
    extends Daedalus.Icons.Block;
    parameter Units.Resistance R=1.2 "Armature resistance";
    parameter Real L(unit="H")=5e-3 "Armature inductance";
    parameter Real k(unit="N.m/A")=0.05 "Torque and back-EMF constant";
    parameter Units.Inertia J=2e-4 "Rotor and load inertia";
    parameter Real b(unit="N.m.s/rad")=1e-5 "Viscous friction";
    parameter Units.Torque tauLoad=0.02 "Load torque applied at t = 1.005 s";
    Daedalus.Blocks.Interfaces.RealInput u "Motor voltage [V]"
      annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    Daedalus.Blocks.Interfaces.RealOutput r "Speed set point [rad/s]"
      annotation (Placement(transformation(extent={{100,50},{120,70}})));
    Daedalus.Blocks.Interfaces.RealOutput y "Measured speed [rad/s]"
      annotation (Placement(transformation(extent={{100,-70},{120,-50}})));
    Units.Current i(start=0, fixed=true) "Armature current";
    Units.AngularVelocity w(start=0, fixed=true) "Rotor speed";
  equation
    L*der(i) = u - R*i - k*w;
    J*der(w) = k*i - b*w - (if time >= 1.005 then tauLoad else 0);
    r = if time < 0.105 then 0 else if time < 1.505 then 100 else 200;
    y = w;
    annotation (Icon(graphics={Text(extent={{-80,40},{80,-40}}, textString="M")}));
  end MotorBench;

  model ClosedLoop "Motor speed loop with the discrete PID: the model-in-the-loop reference"
    extends Daedalus.Icons.Example;
    MotorBench bench annotation (Placement(transformation(extent={{20,-10},{40,10}})));
    DiscretePID pid annotation (Placement(transformation(extent={{-40,-10},{-20,10}})));
  equation
    connect(bench.r, pid.r);
    connect(bench.y, pid.y);
    connect(pid.u, bench.u);
    annotation (experiment(StopTime=3, Interval=0.001),
      Documentation(info="<html>
<p>The same loop runs three ways: here, all in Modelica (model in the loop); with the
generated C of <code>DiscretePID</code> compiled for the host (software in the loop); and
with that C cross-compiled for an Arduino Uno (ATmega328P in simavr) or an ESP32 (in
Espressif's QEMU) while DAEdalus simulates <code>MotorBench</code> and exchanges samples
over the serial port (processor in the loop). See <code>docs/embedded.md</code>.</p>
</html>"));
  end ClosedLoop;

  model ClosedLoopPI "Motor speed loop with the continuous PI"
    extends Daedalus.Icons.Example;
    MotorBench bench;
    SpeedPI pi;
  equation
    connect(bench.r, pi.r);
    connect(bench.y, pi.y);
    connect(pi.u, bench.u);
    annotation (experiment(StopTime=3, Interval=0.001));
  end ClosedLoopPI;
  annotation (Documentation(info="<html>
<p>Controllers and test benches for <code>daedalus codegen-c</code> (embedded C) and
<code>daedalus hil</code> (software and processor in the loop on emulated Arduino and ESP32
boards).</p>
</html>"));
end Embedded;
