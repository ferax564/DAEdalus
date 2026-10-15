within Daedalus.Examples;
package TorqueConverter "Torque converter stall test and vehicle launch (after Claytex)"
  extends Daedalus.Icons.ExamplesPackage;
  import Rot = Daedalus.Mechanics.Rotational;

  model StallTest "Engine at full throttle against a locked turbine: stall speed from the K-factor"
    extends Daedalus.Icons.Example;
    Daedalus.Vehicle.Powertrain.Engine engine(w_start=800*2*3.141592653589793/60);
    Rot.Components.TorqueConverter converter;
    Rot.Components.Fixed brake "Turbine held by the service brake";
    Daedalus.Blocks.Sources.Ramp throttle(height=0.85, offset=0.15, duration=0.5, startTime=0.5) "Idle throttle, then full load";
    Real rpm_engine(unit="rev/min") = engine.rpm;
    Real rpm_stall_theory "Speed where the K-factor curve meets the full-load torque at speed ratio 0";
    Daedalus.Visualizers.World world "Animation: z up";
    Daedalus.Visualizers.Shape engineBlock(r={-0.75, 0, 0.45}, r_shape={0, 0, 0}, length=0.6, width=0.55, height=0.5,
      color={210, 80, 60}) "3-D animation: engine block";
    Daedalus.Visualizers.RotatingDisc impeller(r0={0, 0, 0.45}, diameter=0.42, thickness=0.12, color={160, 165, 175},
      phi=engine.flange.phi) "Converter housing and impeller, turning with the crankshaft";
    Daedalus.Visualizers.RotatingDisc turbine(r0={0.13, 0, 0.45}, diameter=0.36, thickness=0.06, color={95, 100, 110},
      phi=0) "Turbine, held by the brake";
    Daedalus.Visualizers.Shape turbineShaft(shapeType="cylinder", r={0.13, 0, 0.45}, length=0.55, width=0.05, height=0.05,
      color={95, 100, 110}) "Turbine shaft to the brake";
    Daedalus.Visualizers.Shape brakeDrum(shapeType="cylinder", r={0.62, 0, 0.45}, length=0.12, width=0.3, height=0.3,
      color={200, 40, 40}) "Output brake (holds the turbine at stall)";
    Daedalus.Visualizers.Shape bench(r={-1.1, 0, 0}, length=2.0, width=0.9, height=0.04, color={150, 155, 165},
      specularCoefficient=0.2) "Test bench";
    Daedalus.Visualizers.Gauge rpmGauge(r0={0.0, -0.55, 0}, scale=0.8/3000, full=3000, value=engine.rpm)
      "Engine speed, full scale 3000 rpm";
  protected
    parameter Real K0 = converter.table[1, 2];
  equation
    // at stall: tau_engine(rpm) = (rpm/K0)^2, solved on the engine's torque curve by a fixed point
    rpm_stall_theory = K0*sqrt(Daedalus.Blocks.Functions.interpolate(engine.torqueCurve, rpm_engine, 2) - engine.tau_drag*rpm_engine/3000);
    connect(throttle.y, engine.throttle);
    connect(engine.flange, converter.flange_a);
    connect(converter.flange_b, brake.flange);
    annotation (experiment(StopTime=4, Interval=0.004), Documentation(info="<html>
<p>Source: Claytex, <i>Torque converters: the clutches of automatic transmissions</i>,
<a href=\"https://www.claytex.com/tech-blog/torque-converters-the-clutches-of-automatic-transmissions/\">claytex.com/tech-blog</a>.</p>
<p>The stall test is the classic way to read a converter's capacity: with the turbine locked the
speed ratio is zero, and the engine settles where its full-load torque equals the impeller torque
<code>(n/K)^2</code>. Plot <code>rpm_engine</code> against <code>rpm_stall_theory</code>; they converge
to the same stall speed (about 2150 rpm with the default 145 rpm/sqrt(N.m) converter and 250 N.m engine).
The turbine then sees <code>torqueRatio</code> = 2.1 times the engine torque.</p>
<p>Assumed: the converter characteristic in <code>Rotational.Components.TorqueConverter</code> (a generic
passenger-car three-element converter) and a 2.5 l engine torque curve.</p></html>"));
  end StallTest;

  model Driveline "Engine, converter with lock-up clutch, gearbox and vehicle"
    parameter Boolean dynamicConverter=false "true: first-order lag on the converter torque";
    parameter Real gearRatio=4.2*3.2 "First gear times final drive";
    parameter Real lockupRatio=0.88 "Speed ratio at which the lock-up clutch engages";
    parameter Units.Time T_lockup=0.6 "Lock-up clutch engagement ramp";
    Daedalus.Vehicle.Powertrain.Engine engine;
    Rot.Components.TorqueConverter converter(dynamic=dynamicConverter);
    Rot.Components.Clutch lockup(mu=0.12, fn_max=8000, cgeo=0.6, w_small=0.5);
    Rot.Components.Inertia turbine(J=0.05);
    Rot.Components.IdealGear gearbox(ratio=gearRatio, eta=0.96);
    Daedalus.Vehicle.Chassis.VehicleBody car(m=1700, Cd=0.32, A_front=2.4);
    Daedalus.Blocks.Interfaces.RealInput throttle annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    Boolean locked(start=false, fixed=true) "Lock-up command";
    discrete Real t_lock(start=1e30, fixed=true);
    Real v_kmh(unit="km/h") = car.v*3.6;
    parameter Real y_vis=0 "Lane in the 3-D animation" annotation (Dialog(tab="Animation"));
    parameter Real visColor[3]={30, 90, 170} annotation (Dialog(tab="Animation"));
    Daedalus.Visualizers.Car vis(x=car.s, y0=y_vis, color=visColor) "3-D animation (draws only)";
  equation
    when converter.speedRatio > lockupRatio and time > 1 then
      locked = true;
      t_lock = time;
    end when;
    lockup.f_normalized = if locked then min(1, (time - t_lock)/T_lockup) else 0;
    connect(throttle, engine.throttle);
    connect(engine.flange, converter.flange_a);
    connect(converter.flange_b, turbine.flange_a);
    connect(engine.flange, lockup.flange_a);
    connect(lockup.flange_b, turbine.flange_a);
    connect(turbine.flange_b, gearbox.flange_a);
    connect(gearbox.flange_b, car.axle);
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Ellipse(extent={{-80,30},{-20,-30}}, lineColor={0,0,0}, fillColor={175,175,175}, fillPattern=FillPattern.Solid),
        Line(points={{-20,0},{10,0}}, thickness=1),
        Rectangle(extent={{10,30},{40,-30}}, lineColor={0,0,0}, fillColor={160,215,160}, fillPattern=FillPattern.Solid),
        Line(points={{40,0},{60,0}}, thickness=1),
        Ellipse(extent={{55,-40},{85,-70}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{50,-10},{90,-40}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,145},{150,105}}, textString="%name", textColor={0,0,255})}));
  end Driveline;

  model Launch "Full-throttle launch: static K-factor converter versus converter with fluid dynamics"
    extends Daedalus.Icons.Example;
    Daedalus.Visualizers.World world "Animation: z up";
    Driveline static(dynamicConverter=false, y_vis=1.75, visColor={150, 155, 165});
    Driveline dynamic(dynamicConverter=true, y_vis=-1.75, visColor={200, 40, 40});
    Daedalus.Blocks.Sources.Ramp throttle(height=0.85, offset=0.15, duration=0.2, startTime=0.5);
  equation
    connect(throttle.y, static.throttle);
    connect(throttle.y, dynamic.throttle);
    annotation (experiment(StopTime=6, Interval=0.005), Documentation(info="<html>
<p>Source: Claytex, <i>Torque converters: the clutches of automatic transmissions</i>,
<a href=\"https://www.claytex.com/tech-blog/torque-converters-the-clutches-of-automatic-transmissions/\">claytex.com/tech-blog</a>.</p>
<p>The engine flares to near stall speed, the converter multiplies torque while the speed ratio
is low, then approaches the coupling point where the torque ratio is 1. Once the speed ratio passes
<code>lockupRatio</code> the lock-up clutch closes and engine speed is pulled down to turbine speed,
removing the converter slip loss (<code>converter.lossPower</code>).
The <code>dynamic</code> driveline adds the fluid's torque build-up lag, which delays the
initial torque at the wheels and softens the flare. Plot <code>static.engine.rpm</code>,
<code>static.converter.speedRatio</code>, <code>static.v_kmh</code> and the same signals of
<code>dynamic</code>.</p>
<p>Assumed: 1700 kg car, first gear 4.2 and final drive 3.2, a 0.6 s lock-up ramp and a 50 ms fluid time constant.</p></html>"));
  end Launch;
end TorqueConverter;
