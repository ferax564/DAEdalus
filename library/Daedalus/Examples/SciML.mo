within Daedalus.Examples;
package SciML "Hybrid physics and machine learning: a DC motor with friction nobody modelled"
  extends Daedalus.Icons.ExamplesPackage;

  model MotorTruth "Stands in for the test rig: a DC motor with Coulomb friction and drag"
    parameter Real R(unit="Ohm")=1.2 "Armature resistance";
    parameter Real L(unit="H")=5e-3 "Armature inductance";
    parameter Real k(unit="N.m/A")=0.05 "Motor constant";
    parameter Real J(unit="kg.m2")=2e-4 "Rotor inertia";
    parameter Real b(unit="N.m.s/rad")=1e-5 "Viscous friction (known)";
    parameter Real Tc(unit="N.m")=4e-3 "Coulomb friction (not in the known model)";
    parameter Real cd(unit="N.m.s2/rad2")=2e-7 "Quadratic drag (not in the known model)";
    parameter Real wc(unit="rad/s")=2 "Smoothing speed of the Coulomb friction";
    Real u(unit="V") "Voltage profile of the experiment";
    Real i(unit="A", start=0, fixed=true);
    Real w(unit="rad/s", start=0, fixed=true);
    Real tau_f(unit="N.m") "The friction the known model lacks";
  equation
    u = 6*sin(2*3.141592653589793*0.4*time) + 3*sin(2*3.141592653589793*1.3*time);
    L*der(i) = u - R*i - k*w;
    J*der(w) = k*i - b*w - tau_f;
    tau_f = Tc*tanh(w/wc) + cd*w*abs(w);
    annotation (experiment(StopTime=5, Interval=0.002));
  end MotorTruth;

  model MotorKnown "The physics we trust, with the unknown friction torque as an input"
    parameter Real R(unit="Ohm")=1.2;
    parameter Real L(unit="H")=5e-3;
    parameter Real k(unit="N.m/A")=0.05;
    parameter Real J(unit="kg.m2")=2e-4;
    parameter Real b(unit="N.m.s/rad")=1e-5;
    input Real tau_missing(unit="N.m") "Unknown torque, found from data";
    Real u(unit="V");
    Real i(unit="A", start=0, fixed=true);
    Real w(unit="rad/s", start=0, fixed=true);
  equation
    u = 6*sin(2*3.141592653589793*0.4*time) + 3*sin(2*3.141592653589793*1.3*time);
    L*der(i) = u - R*i - k*w;
    J*der(w) = k*i - b*w - tau_missing;
    annotation (experiment(StopTime=5, Interval=0.002));
  end MotorKnown;

  model MotorDiscovered "The known model with the friction law recovered by symbolic regression"
    MotorKnown m(tau_missing=0.004*tanh(m.w/2.0) + 2e-07*m.w*abs(m.w));
    annotation (Documentation(info="<html>
<p>The binding of <code>tau_missing</code> is what <code>python/examples/sciml_motor.py</code>
prints: run it to regenerate this line from the data of <b>MotorTruth</b>.</p>
</html>"), experiment(StopTime=5, Interval=0.002));
  end MotorDiscovered;

  annotation (Documentation(info="<html>
<p>A worked example of scientific machine learning with DAEdalus models, using
<code>daedalus.sciml</code> (NumPy only):</p>
<ol>
<li><b>MotorTruth</b> generates the \"measured\" speed and current (with noise added in Python).</li>
<li><b>MotorKnown</b> holds the physics we trust and a top-level input <code>tau_missing</code> for the
    torque we cannot explain. <code>sciml.missing_term</code> finds its value along the data from the
    model's right-hand side.</li>
<li><code>sciml.sindy</code> turns those values into a formula over candidate terms
    (<code>tanh(w/2)</code>, <code>w*abs(w)</code>, ...), which goes back into the model as
    <b>MotorDiscovered</b>.</li>
<li><code>sciml.fit_surrogate</code> learns a neural surrogate of the motor from its parameters and
    exports it as a Modelica block that simulates and compiles to embedded C.</li>
</ol>
</html>"));
end SciML;
