within MultiBodyVehicle;
package Examples "Suspension rig and full-vehicle manoeuvres"
  extends Modelica.Icons.ExamplesPackage;
  import SI = Modelica.Units.SI;
  import MB = Modelica.Mechanics.MultiBody;

  model SuspensionKinematics "Double wishbone vs MacPherson: camber, track and motion ratio over +-80 mm wheel travel"
    extends Modelica.Icons.Example;
    parameter SI.Angle phi_sweep=0.17 "Lower-arm sweep amplitude";
    parameter SI.Time T_sweep=4 "Sweep duration (quasi-static)";
    inner MB.World world(n={0,0,-1}, enableAnimation=false)
      annotation (Placement(transformation(extent={{-90,-10},{-70,10}})));
    Components.DoubleWishbone doubleWishbone(steerable=false, lowerPivot(phi(start=-phi_sweep, fixed=true), w(start=2*phi_sweep/T_sweep, fixed=true)))
      annotation (Placement(transformation(extent={{-20,20},{0,40}})));
    Components.MacPherson macPherson(lowerPivot(phi(start=-phi_sweep, fixed=true), w(start=2*phi_sweep/T_sweep, fixed=true)))
      annotation (Placement(transformation(extent={{-20,-40},{0,-20}})));
    Modelica.Mechanics.Rotational.Sources.ConstantSpeed sweepDW(w_fixed=2*phi_sweep/T_sweep, phi(start=-phi_sweep, fixed=true))
      "Rig actuator: turns the lower arm from rebound to bump"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,-10})));
    Modelica.Mechanics.Rotational.Sources.ConstantSpeed sweepMP(w_fixed=2*phi_sweep/T_sweep, phi(start=-phi_sweep, fixed=true))
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,-80})));
    Modelica.Mechanics.Rotational.Components.SpringDamper rigDW(c=2e5, d=2e3) "Stiff rig coupling"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,10})));
    Modelica.Mechanics.Rotational.Components.SpringDamper rigMP(c=2e5, d=2e3) "Stiff rig coupling"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,-55})));
    Real MR_doubleWishbone = -der(doubleWishbone.L_spring)/der(doubleWishbone.travel)
      "Motion ratio: spring compression per unit wheel travel";
    Real MR_macPherson = -der(macPherson.L_strut)/der(macPherson.travel)
      "Motion ratio: strut compression per unit wheel travel";
    Real cg_doubleWishbone(unit="deg/m") = der(doubleWishbone.camber)/der(doubleWishbone.travel) "Camber gain";
    Real cg_macPherson(unit="deg/m") = der(macPherson.camber)/der(macPherson.travel) "Camber gain";
  equation
    connect(world.frame_b, doubleWishbone.frame_a);
    connect(world.frame_b, macPherson.frame_a);
    connect(sweepDW.flange, rigDW.flange_a);
    connect(rigDW.flange_b, doubleWishbone.flange_arm);
    connect(sweepMP.flange, rigMP.flange_a);
    connect(rigMP.flange_b, macPherson.flange_arm);
    annotation (experiment(StopTime=4, Interval=0.004), Documentation(info="<html>
<p>A kinematic test rig. The body is fixed to the world and the lower arm of each corner is
swept at constant angular speed (through a stiff rig coupling) from rebound to bump (about -80 to +80 mm wheel travel), slowly
enough to be quasi-static. Plot against <code>*.travel</code>:</p>
<ul>
<li><code>*.camber</code>: the short-long-arm double wishbone gains negative camber in bump;
    the MacPherson strut gains much less because its upright rotates with the long strut.</li>
<li><code>*.track_change</code>: lateral scrub of the wheel centre.</li>
<li><code>MR_*</code>: spring-to-wheel motion ratio. The wheel rate is <code>c_spring*MR^2</code>.</li>
</ul>
</html>"));
  end SuspensionKinematics;

  model QuarterCarBump "Multibody double-wishbone quarter car vs its 1-D equivalent over a 50 mm bump"
    extends Modelica.Icons.Example;
    parameter SI.Mass m_quarter=325 "Quarter of the sprung mass";
    parameter SI.Velocity v=10 "Road speed";
    parameter Real MR=0.65 "Design motion ratio (from SuspensionKinematics)";
    final parameter SI.TranslationalSpringConstant k_wheel=corner.c_spring*MR^2 "Equivalent wheel rate";
    final parameter SI.TranslationalDampingConstant d_wheel=corner.d_damper*MR^2 "Equivalent wheel damping";
    final parameter SI.Mass m_unsprung=corner.m_wheel + corner.m_upright + (corner.m_lowerArm + corner.m_upperArm)/2
      "Wheel, upright and half of each arm";
    final parameter SI.Mass m_sprung=m_quarter + (corner.m_lowerArm + corner.m_upperArm)/2 "Body and the other half of the arms";
    final parameter SI.Height z0=tyre.R0 - (m_sprung + m_unsprung)*Modelica.Constants.g_n/tyre.c_z "Static ride height";
    inner MB.World world(n={0,0,-1}, enableAnimation=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    MB.Joints.Prismatic heave(n={0,0,1}, s(start=z0, fixed=true), v(start=0, fixed=true), stateSelect=StateSelect.always)
      "Body guided vertically"
      annotation (Placement(transformation(extent={{-60,30},{-40,50}})));
    MB.Parts.Body quarterBody(m=m_quarter, r_CM={0,0,0}, animation=false)
      annotation (Placement(transformation(extent={{-30,60},{-10,80}})));
    Components.DoubleWishbone corner(steerable=false, F_spring_design=m_sprung*Modelica.Constants.g_n/MR,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-20,30},{0,50}})));
    Components.Tyre tyre(v_belt=v, bump_h=0.05, bump_L=0.5, bump_x0=2)
      "Flat-belt rig: the road moves at v; the bump reaches the wheel at t = 0.2 s"
      annotation (Placement(transformation(extent={{20,10},{40,30}})));
    Modelica.Blocks.Sources.Constant noDrive(k=0)
      annotation (Placement(transformation(extent={{-10,0},{10,20}})));

    Modelica.Mechanics.Translational.Components.Fixed ground
      annotation (Placement(transformation(extent={{50,-90},{70,-70}})));
    Modelica.Mechanics.Translational.Sources.Position road(exact=false, f_crit=500)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={60,-60})));
    Modelica.Blocks.Sources.RealExpression roadProfile(y=Components.roadHeight(v*time, 0, 0.05, 0.5, 2, -1, 1))
      annotation (Placement(transformation(extent={{20,-70},{40,-50}})));
    Modelica.Mechanics.Translational.Components.ElastoGap tyre1D(c=tyre.c_z, d=tyre.d_z, s_rel0=tyre.R0)
      "Tyre that can leave the road"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={60,-30})));
    Modelica.Mechanics.Translational.Components.Mass wheel1D(m=m_unsprung, s(start=z0, fixed=true), v(start=0, fixed=true))
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={60,0})));
    Modelica.Mechanics.Translational.Components.SpringDamper suspension1D(c=k_wheel, d=d_wheel,
      s_rel0=0.3 + m_sprung*Modelica.Constants.g_n/k_wheel)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={60,30})));
    Modelica.Mechanics.Translational.Components.Mass body1D(m=m_sprung, s(start=z0 + 0.3, fixed=true), v(start=0, fixed=true))
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={60,60})));
    Modelica.Mechanics.Translational.Sources.Force gravity1D_body
      annotation (Placement(transformation(extent={{100,50},{80,70}})));
    Modelica.Mechanics.Translational.Sources.Force gravity1D_wheel
      annotation (Placement(transformation(extent={{100,-10},{80,10}})));
    Modelica.Blocks.Sources.Constant wBody(k=-m_sprung*Modelica.Constants.g_n)
      annotation (Placement(transformation(extent={{130,50},{110,70}})));
    Modelica.Blocks.Sources.Constant wWheel(k=-m_unsprung*Modelica.Constants.g_n)
      annotation (Placement(transformation(extent={{130,-10},{110,10}})));

    SI.Position z_body = heave.s - z0 "Multibody: body displacement from static";
    SI.Position z_body1D = body1D.s - (z0 + 0.3) "1-D equivalent: body displacement from static";
    SI.Acceleration a_body = der(heave.v) "Multibody: body acceleration";
    SI.Acceleration a_body1D = body1D.a "1-D equivalent: body acceleration";
    SI.Force F_tyre = tyre.Fz "Multibody: tyre load";
    SI.Force F_tyre1D = -tyre1D.f "1-D equivalent: tyre load";
    SI.Length travel = corner.travel "Multibody: wheel travel";
    SI.Length travel1D = 0.3 - (body1D.s - wheel1D.s) "1-D equivalent: suspension compression";
  equation
    connect(world.frame_b, heave.frame_a);
    connect(heave.frame_b, quarterBody.frame_a);
    connect(heave.frame_b, corner.frame_a);
    connect(corner.frame_wheel, tyre.frame_a);
    connect(noDrive.y, tyre.F_drive);
    connect(ground.flange, road.support);
    connect(roadProfile.y, road.s_ref);
    connect(road.flange, tyre1D.flange_a);
    connect(tyre1D.flange_b, wheel1D.flange_a);
    connect(wheel1D.flange_b, suspension1D.flange_a);
    connect(suspension1D.flange_b, body1D.flange_a);
    connect(wBody.y, gravity1D_body.f);
    connect(gravity1D_body.flange, body1D.flange_b);
    connect(wWheel.y, gravity1D_wheel.f);
    connect(gravity1D_wheel.flange, wheel1D.flange_b);
    annotation (experiment(StopTime=2, Interval=0.001), Documentation(info="<html>
<p>The 3-D double-wishbone corner carries a quarter of the body on a vertical guide while a
flat-belt road moves under the wheel at 10 m/s and carries it over a 50 mm, 0.5 m half-cosine bump
(the same input as <code>Daedalus.Examples.QuarterCar.BumpComparison</code>).</p>
<p>Next to it is the classic 1-D two-mass quarter car, using the wheel rate and wheel damping
from the design motion ratio (<code>c*MR^2</code>, <code>d*MR^2</code>), the same tyre (which can
also leave the road) and the same split of sprung and unsprung mass (half of each arm on each side).
Compare <code>z_body</code> with <code>z_body1D</code>, <code>travel</code> with <code>travel1D</code>
and <code>F_tyre</code> with <code>F_tyre1D</code>. Peak travel agrees to 0.1 mm and peak tyre load
to about 2 %; what remains is the geometry the 1-D model leaves out, such as the lateral tyre scrub
from track change and the moving arm inertias.</p>
</html>"));
  end QuarterCarBump;

  model StepSteer "Full car: 2 deg front-wheel step steer at 72 km/h"
    extends Modelica.Icons.Example;
    parameter SI.Angle delta_step=Modelica.Units.Conversions.from_deg(2) "Road-wheel steer angle";
    inner MB.World world(n={0,0,-1}, enableAnimation=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    Components.Vehicle car(v_start=20)
      annotation (Placement(transformation(extent={{0,-20},{40,20}})));
    Modelica.Blocks.Sources.Ramp steer(height=delta_step, duration=0.2, startTime=1)
      annotation (Placement(transformation(extent={{-60,-10},{-40,10}})));
    Real yawGain(unit="1/s") = Modelica.Units.Conversions.from_deg(car.yawRate)/max(steer.y, 1e-3)
      "Yaw-rate gain r/delta";
  equation
    connect(steer.y, car.delta);
    annotation (experiment(StopTime=5, Interval=0.005), Documentation(info="<html>
<p>The car settles for 1 s at 20 m/s, then the front wheels are steered 2 deg to the left over
0.2 s. Plot <code>car.yawRate</code>, <code>car.a_y</code>, <code>car.roll</code>,
<code>car.sideslip</code> and the four tyre loads <code>car.t*.Fz</code> (lateral load transfer to
the right-hand wheels).</p>
<p>Check: the tyre cornering stiffness <code>B*C*mu*Fz</code> is proportional to load, so a linear
bicycle model of this car is neutral steer, with a steady yaw-rate gain of
<code>v/L</code> = 7.7 1/s. The multibody result <code>yawGain</code> is 7.5 1/s for a 0.1 deg step
and 7.3 1/s for the 2 deg step: mild understeer that grows with lateral acceleration.</p>
</html>"));
  end StepSteer;

  model LaneChange "Full car: single-sine lane change at 72 km/h"
    extends Modelica.Icons.Example;
    parameter SI.Angle delta_max=Modelica.Units.Conversions.from_deg(2) "Peak road-wheel angle";
    parameter SI.Time T=2 "Steering period";
    inner MB.World world(n={0,0,-1}, enableAnimation=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    Components.Vehicle car(v_start=20)
      annotation (Placement(transformation(extent={{0,-20},{40,20}})));
    Modelica.Blocks.Sources.RealExpression steer(y=if time > 1 and time < 1 + T then delta_max*sin(2*Modelica.Constants.pi*(time - 1)/T) else 0)
      annotation (Placement(transformation(extent={{-60,-10},{-40,10}})));
    SI.Position y_lateral = car.body.frame_a.r_0[2] "Lateral displacement";
  equation
    connect(steer.y, car.delta);
    annotation (experiment(StopTime=6, Interval=0.005), Documentation(info="<html>
<p>One full sine period of steering (2 deg peak, 2 s) at 20 m/s moves the car one lane
sideways. Plot <code>y_lateral</code>, <code>car.yawRate</code>, <code>car.a_y</code> and
<code>car.roll</code>; the roll and lateral acceleration lag the steering input, and the car ends
on a new heading.</p>
</html>"));
  end LaneChange;

  model BumpLeft "Full car: left wheels cross a 50 mm bump at 36 km/h"
    extends Modelica.Icons.Example;
    inner MB.World world(n={0,0,-1}, enableAnimation=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    Components.Vehicle car(v_start=10, bump_h=0.05, bump_L=0.8, bump_x0=10, bump_yMin=0, bump_yMax=5)
      annotation (Placement(transformation(extent={{0,-20},{40,20}})));
    Modelica.Blocks.Sources.Constant straight(k=0)
      annotation (Placement(transformation(extent={{-60,-10},{-40,10}})));
  equation
    connect(straight.y, car.delta);
    annotation (experiment(StopTime=3, Interval=0.002), Documentation(info="<html>
<p>Driving straight at 10 m/s, the left-hand wheels cross a 50 mm half-cosine bump 0.8 m long.
The front-left wheel reaches it at 1 s and the rear-left 0.26 s later (wheelbase / speed).
Plot <code>car.FL.travel</code> and <code>car.RL.travel</code> (wheel travel),
<code>car.roll</code>, <code>car.pitch</code>, <code>car.heave</code> and the tyre loads
<code>car.tFL.Fz</code>, <code>car.tRL.Fz</code>. The front-left tyre briefly unloads behind the
bump crest; the right-hand wheels feel the bump only through the body.</p>
</html>"));
  end BumpLeft;
end Examples;
