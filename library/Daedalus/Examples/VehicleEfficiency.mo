within Daedalus.Examples;
package VehicleEfficiency "EV drive-cycle energy studies (after Claytex VeSyMA and Modelon)"
  extends Daedalus.Icons.ExamplesPackage;
  import V = Daedalus.Vehicle;
  import Rot = Daedalus.Mechanics.Rotational;

  model ElectricVehicle "Battery-electric car: driver, 350 N.m machine, single-speed reduction, body, battery"
    parameter Real cycle[:, 2]=V.DriveCycles.NEDC "Drive cycle [s, km/h]";
    parameter Integer repetitions=1;
    parameter Units.Mass m=1650 "Kerb mass + driver";
    parameter Units.Mass m_payload=0 "Additional payload";
    parameter Real Cd=0.29;
    parameter Units.Area A_front=2.3;
    parameter Units.Area dCdA=0 "Additional drag area Cd*A (e.g. a roof box)";
    parameter Real crr=0.009;
    parameter Real grade=0;
    parameter Units.Velocity v_wind=0;
    parameter Units.Angle psi_wind=0;
    parameter Real ratio=9.0 "Reduction gear";
    V.DriveCycles.DriveCycle driveCycle(table=cycle, repetitions=repetitions);
    V.Drivers.Driver driver;
    Daedalus.Blocks.Math.Gain torqueRequest(k=motor.tau_max);
    V.Powertrain.ElectricMachine motor(tau_max=350, P_max=150e3);
    Rot.Components.IdealGear reduction(ratio=ratio, eta=0.97);
    V.Chassis.VehicleBody body(m=m + m_payload, Cd=Cd + dCdA/A_front, A_front=A_front, crr=crr, grade=grade,
      v_wind=v_wind, psi_wind=psi_wind);
    V.Powertrain.Battery battery(capacity=200, SOC_start=0.95);
    V.Sensors.EnergyMeter meter;
    Real v_kmh(unit="km/h") = body.v*3.6;
    Real distance_km = body.s/1000;
  equation
    connect(driveCycle.y, driver.v_ref);
    connect(body.v, driver.v);
    connect(driver.pedal, torqueRequest.u);
    connect(torqueRequest.y, motor.tau_ref);
    connect(motor.flange, reduction.flange_a);
    connect(reduction.flange_b, body.axle);
    connect(motor.P_el, battery.P);
    connect(motor.P_el, meter.P);
    connect(body.s, meter.s);
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Polygon(points={{-80,-30},{-80,0},{-50,10},{-25,40},{35,40},{60,10},{80,5},{80,-30},{-80,-30}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-60,-15},{-30,-45}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{35,-15},{65,-45}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-20,-50},{20,-75}}, lineColor={0,0,0}, fillColor={0,170,0}, fillPattern=FillPattern.Solid),
        Text(extent={{-20,-50},{20,-75}}, textString="+ -", textColor={255,255,255}),
        Text(extent={{-150,145},{150,105}}, textString="%name", textColor={0,0,255})}));
  end ElectricVehicle;

  model NEDCSensitivity "Baseline, 2 % grade, 25 mph crosswind at 30 deg and a roof box over repeated NEDC"
    extends Daedalus.Icons.Example;
    parameter Integer repetitions=11 "About 118 km";
    ElectricVehicle baseline(repetitions=repetitions);
    ElectricVehicle uphill(repetitions=repetitions, grade=0.02);
    ElectricVehicle crosswind(repetitions=repetitions, v_wind=25*0.44704, psi_wind=30*3.141592653589793/180);
    ElectricVehicle roofBox(repetitions=repetitions, m_payload=50, dCdA=0.12);
    Real delta_uphill(unit="%") = (uphill.meter.Wh_per_km/baseline.meter.Wh_per_km - 1)*100 "Consumption change [%]";
    Real delta_crosswind(unit="%") = (crosswind.meter.Wh_per_km/baseline.meter.Wh_per_km - 1)*100 "Consumption change [%]";
    Real delta_roofBox(unit="%") = (roofBox.meter.Wh_per_km/baseline.meter.Wh_per_km - 1)*100 "Consumption change [%]";
    annotation (experiment(StopTime=12980, Interval=1), Documentation(info="<html>
<p>Source: Claytex, <i>Sensitivity testing of vehicle efficiency using VeSyMA drive cycle</i>,
<a href=\"https://www.claytex.com/tech-blog/sensitivity-testing-of-vehicle-efficiency-using-vesyma-drive-cycle/\">claytex.com/tech-blog</a>.</p>
<p>Four copies of the same electric car drive the NEDC back to back, 11 times. One runs on the flat
in still air, one on a constant 2 % grade, one with a 25 mph (11.2 m/s) wind at 30 degrees to the
direction of travel (the yaw angle raises the drag coefficient as <code>Cd*(1 + k_yaw*beta^2)</code>),
and one with a roof box (+50 kg, +0.12 m2 drag area). Compare <code>*.meter.Wh_per_km</code>,
<code>*.battery.SOC</code> and the <code>delta_*</code> percentages at the end of the run.</p>
<p>Assumed: 1650 kg car, Cd 0.29, 2.3 m2, crr 0.009, 350 N.m / 150 kW machine with a simple copper/iron
loss model, 9:1 reduction, 75 kWh pack. The NEDC table is the regulation's segment definition
(see <code>Vehicle.DriveCycles.NEDC</code>).</p></html>"));
  end NEDCSensitivity;

  model CoastDown "Coast-down from 130 km/h in neutral; the result is fitted to F = F0 + F1*v + F2*v^2"
    extends Daedalus.Icons.Example;
    V.Chassis.VehicleBody body(m=1650, Cd=0.29, A_front=2.3, crr=0.009, v_start=130/3.6);
    Real v_kmh(unit="km/h") = body.v*3.6;
    Units.Force F_decel = -(body.m + body.J_wheels/body.R_wheel^2)*body.a "Road load from the deceleration";
  equation
    // body.axle is left unconnected: its torque is zero (neutral gear).
    when body.v < 15/3.6 then
      terminate("coast-down reached 15 km/h");
    end when;
    annotation (experiment(StopTime=600, Interval=0.5), Documentation(info="<html>
<p>Source: Modelon, <i>Advancing your vehicle's energy-efficient design</i>,
<a href=\"https://www.modelon.com/blog/advancing-your-vehicles-energy-efficient-design/\">modelon.com/blog</a>.</p>
<p>The vehicle coasts with the driveline open, as in an SAE J2263 / WLTP coast-down test. The road load
<code>F_decel</code> recovered from the deceleration is fitted with
<code>tools/fit_coastdown.py</code> to the dynamometer form <code>F0 + F1*v + F2*v^2</code>; the fitted
coefficients feed <code>UDDSEnergy</code>.</p></html>"));
  end CoastDown;

  model UDDSEnergy "UDDS energy with the physical body versus fitted road-load coefficients"
    extends Daedalus.Icons.Example;
    ElectricVehicle physical(cycle=V.DriveCycles.UDDS);
    V.DriveCycles.DriveCycle driveCycle(table=V.DriveCycles.UDDS);
    V.Drivers.Driver driver;
    Daedalus.Blocks.Math.Gain torqueRequest(k=motor.tau_max);
    V.Powertrain.ElectricMachine motor(tau_max=350, P_max=150e3);
    Rot.Components.IdealGear reduction(ratio=9.0, eta=0.97);
    V.Chassis.RoadLoadVehicle fitted(m=1650, F0=145.7, F1=0.0, F2=0.4002) "Coefficients from tools/fit_coastdown.py";
    V.Sensors.EnergyMeter meter;
    Real energy_difference(unit="%") = (meter.Wh_per_km/physical.meter.Wh_per_km - 1)*100 "[%]";
  equation
    connect(driveCycle.y, driver.v_ref);
    connect(fitted.v, driver.v);
    connect(driver.pedal, torqueRequest.u);
    connect(torqueRequest.y, motor.tau_ref);
    connect(motor.flange, reduction.flange_a);
    connect(reduction.flange_b, fitted.axle);
    connect(motor.P_el, meter.P);
    connect(fitted.s, meter.s);
    annotation (experiment(StopTime=1369, Interval=0.5), Documentation(info="<html>
<p>Source: Modelon, <i>Advancing your vehicle's energy-efficient design</i>,
<a href=\"https://www.modelon.com/blog/advancing-your-vehicles-energy-efficient-design/\">modelon.com/blog</a>.</p>
<p>The same car drives the EPA UDDS twice: once with the physical body (drag, rolling resistance) and
once with the road-load coefficients fitted from <code>CoastDown</code>. If the fit is good the two
consumptions agree to well under one percent (<code>energy_difference</code>). The UDDS table comes
from the EPA schedule as distributed with NREL FASTSim.</p></html>"));
  end UDDSEnergy;
end VehicleEfficiency;
