within Daedalus.Examples;
package OptimalControl "Plant models for optimal control, MPC and moving horizon estimation with CasADi"
  extends Daedalus.Icons.ExamplesPackage;
  import Tr = Daedalus.Mechanics.Translational;
  import Rot = Daedalus.Mechanics.Rotational;
  import V = Daedalus.Vehicle;

  model LineForce "Force between two flanges set by a signal: positive pushes the flanges apart"
    extends Tr.Interfaces.PartialCompliant;
    Daedalus.Blocks.Interfaces.RealInput F(unit="N") "Actuator force"
      annotation (Placement(transformation(origin={0,120}, extent={{-20,-20},{20,20}}, rotation=270)));
  equation
    f = -F;
    annotation (Icon(graphics={
        Rectangle(extent={{-60,30},{60,-30}}, lineColor={0,0,0}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
        Line(points={{-100,0},{-60,0}}, color={0,127,0}),
        Line(points={{60,0},{100,0}}, color={0,127,0}),
        Polygon(points={{-50,0},{-20,15},{-20,-15},{-50,0}}, fillColor={0,127,0}, fillPattern=FillPattern.Solid),
        Polygon(points={{50,0},{20,15},{20,-15},{50,0}}, fillColor={0,127,0}, fillPattern=FillPattern.Solid)}));
  end LineForce;

  model AileronServo "Aileron actuator with the servo-valve command as a top-level input (no position loop)"
    extends Daedalus.Icons.Example;
    import AA = Daedalus.Examples.AileronActuator;
    input Real u(min=-1, max=1) "Normalised valve command";
    parameter Units.Length r_horn=0.08 "Control horn lever arm";
    parameter Real K_hinge(unit="N.m/rad")=5e3 "Aerodynamic hinge-moment stiffness";
    AA.HydraulicSupply hyd;
    AA.Actuator act;
    Tr.Components.Fixed wingSpar(s0=-act.stroke/2) "Rod at 0 with the actuator at mid-stroke";
    Rot.Components.IdealLever horn(r=r_horn);
    Rot.Components.Inertia aileron(J=1.5) "Aileron about its hinge line";
    Rot.Components.SpringDamper hingeMoment(c=K_hinge, d=50) "Aerodynamic hinge moment (linearised)";
    Rot.Components.Fixed hinge;
    output Real delta_deg = aileron.phi*180/3.141592653589793 "Aileron deflection";
    Real P_hyd(unit="W") = (hyd.P.p - hyd.T.p)*abs(act.cylinder.port_A.q) "Hydraulic power drawn (approximate)";
  equation
    act.u = u;
    connect(hyd.P, act.P);
    connect(act.T, hyd.T);
    connect(wingSpar.flange, act.mount);
    connect(act.rod, horn.flange_b);
    connect(aileron.flange_a, horn.flange_a);
    connect(hingeMoment.flange_a, hinge.flange);
    connect(hingeMoment.flange_b, aileron.flange_b);
    annotation (experiment(StopTime=0.5, Interval=0.001), Documentation(info="<html>
<p>The plant of <code>AileronActuator.PositionControl</code> with the proportional position loop
removed: the servo-valve command <code>u</code> (between -1 and 1) is a top-level input, so an optimal
control problem can choose it. <code>python/examples/ocp_aileron.py</code> finds the fastest
10 degree deflection that ends at rest, with the valve command bounded.</p></html>"));
  end AileronServo;

  model ActiveSuspension "Quarter car with a force actuator in parallel with a soft passive damper"
    extends Daedalus.Icons.Example;
    input Real F_act(unit="N") "Actuator force, positive pushes body and wheel apart";
    parameter Units.Mass m_body=400 "Quarter of the body mass";
    parameter Units.Mass m_wheel=40 "Wheel, hub and brake";
    parameter Units.TranslationalSpringConstant c_spring=30e3;
    parameter Units.TranslationalSpringConstant c_tyre=250e3;
    parameter Units.TranslationalDampingConstant d_tyre=100;
    parameter Units.TranslationalDampingConstant d_passive=800 "Soft passive damper";
    Tr.Components.Mass body(m=m_body, g=-9.81, s(start=0.6, fixed=true), v(start=0, fixed=true));
    Tr.Components.Mass wheel(m=m_wheel, g=-9.81, s(start=0.3, fixed=true), v(start=0, fixed=true));
    Tr.Components.Spring spring(c=c_spring, s_rel0=0.3 + m_body*9.81/c_spring) "Preloaded to carry the body";
    Tr.Components.Damper damper(d=d_passive);
    LineForce actuator;
    Tr.Components.SpringDamper tyre(c=c_tyre, d=d_tyre, s_rel0=0.3 + (m_body + m_wheel)*9.81/c_tyre)
      "Tyre in permanent road contact";
    Tr.Sources.Position road(f_crit=200);
    Daedalus.Examples.QuarterCar.RoadBump bump(h=0.05, L=0.5, v=10, t0=0.2);
    Units.Acceleration a_body = body.a "Vertical body acceleration (ride comfort)";
    Units.Length travel = spring.s_rel - 0.3 "Suspension travel from the static position";
    Units.Force F_tyre = -tyre.f "Tyre load (road holding)";
  equation
    F_act = actuator.F;
    connect(bump.y, road.s_ref);
    connect(road.flange, tyre.flange_a);
    connect(tyre.flange_b, wheel.flange_a);
    connect(wheel.flange_b, spring.flange_a);
    connect(spring.flange_b, body.flange_a);
    connect(wheel.flange_b, damper.flange_a);
    connect(damper.flange_b, body.flange_a);
    connect(wheel.flange_b, actuator.flange_a);
    connect(actuator.flange_b, body.flange_a);
    annotation (experiment(StopTime=1.0, Interval=0.001), Documentation(info="<html>
<p>A quarter car crossing the 50 mm bump of <code>QuarterCar.BumpComparison</code>, with a force actuator
between wheel and body and a soft passive damper. The actuator force <code>F_act</code> is a top-level
input. <code>python/examples/mpc_active_suspension.py</code> closes the loop with a model predictive
controller that previews the bump and trades body acceleration against suspension travel and actuator
force.</p></html>"));
  end ActiveSuspension;

  model CoastDownTest "Coast-down from 130 km/h in neutral, for estimating the road load from speed readings"
    extends Daedalus.Icons.Example;
    V.Chassis.VehicleBody body(m=1650, Cd=0.29, A_front=2.3, crr=0.009, v_start=130/3.6);
    output Real v_kmh(unit="km/h") = body.v*3.6 "Measured speed";
    annotation (experiment(StopTime=240, Interval=1), Documentation(info="<html>
<p><code>VehicleEfficiency.CoastDown</code> without the stop at 15 km/h. Only the speed is measured.
<code>python/examples/mhe_coastdown.py</code> generates noisy speed readings by simulating this model
in DAEdalus with a different drag and rolling-resistance coefficient, then runs a moving horizon
estimator that recovers the speed and both coefficients (<code>body.Cd</code>, <code>body.crr</code>).</p></html>"));
  end CoastDownTest;
end OptimalControl;
