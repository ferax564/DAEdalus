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
    Real rpm_engine = engine.rpm;
    Real rpm_stall_theory "Speed where the K-factor curve meets the full-load torque at speed ratio 0";
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
    Daedalus.Blocks.Interfaces.RealInput throttle;
    Boolean locked(start=false, fixed=true) "Lock-up command";
    discrete Real t_lock(start=1e30, fixed=true);
    Real v_kmh = car.v*3.6;
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
  end Driveline;

  model Launch "Full-throttle launch: static K-factor converter versus converter with fluid dynamics"
    extends Daedalus.Icons.Example;
    Driveline static(dynamicConverter=false);
    Driveline dynamic(dynamicConverter=true);
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
