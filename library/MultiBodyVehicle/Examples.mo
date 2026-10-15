within MultiBodyVehicle;
package Examples "Suspension rig and full-vehicle manoeuvres"
  extends Modelica.Icons.ExamplesPackage;
  import SI = Modelica.Units.SI;
  import MB = Modelica.Mechanics.MultiBody;

  model SuspensionKinematics "Double wishbone vs MacPherson: camber, track and motion ratio over +-80 mm wheel travel"
    extends Modelica.Icons.Example;
    parameter SI.Angle phi_sweep=0.17 "Lower-arm sweep amplitude";
    parameter SI.Time T_sweep=4 "Sweep duration (quasi-static)";
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
      annotation (Placement(transformation(extent={{-90,-10},{-70,10}})));
    Components.DoubleWishbone doubleWishbone(steerable=false, lowerPivot(phi(start=-phi_sweep, fixed=true), w(start=2*phi_sweep/T_sweep, fixed=true)))
      annotation (Placement(transformation(extent={{-20,20},{0,40}})));
    Components.MacPherson macPherson(lowerPivot(phi(start=-phi_sweep, fixed=true), w(start=2*phi_sweep/T_sweep, fixed=true)))
      annotation (Placement(transformation(extent={{-20,-40},{0,-20}})));
    MB.Parts.FixedTranslation rigOffset(r={-1.2, 0, 0}, animation=false)
      "Places the MacPherson rig behind the double wishbone, so the two do not overlap in the animation"
      annotation (Placement(transformation(extent={{-60,-40},{-40,-20}})));
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
    connect(world.frame_b, rigOffset.frame_a);
    connect(rigOffset.frame_b, macPherson.frame_a);
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

  model PushrodVsCoilOver "Double wishbone with an outboard coil-over vs a pushrod and rocker with an inboard coil-over"
    extends Modelica.Icons.Example;
    parameter SI.Angle phi_sweep=0.17 "Lower-arm sweep amplitude";
    parameter SI.Time T_sweep=4 "Sweep duration (quasi-static)";
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
      annotation (Placement(transformation(extent={{-90,-10},{-70,10}})));
    Components.DoubleWishbone coilOver(steerable=false, lowerPivot(phi(start=-phi_sweep, fixed=true), w(start=2*phi_sweep/T_sweep, fixed=true)))
      "External spring: coil-over from the lower arm to the body"
      annotation (Placement(transformation(extent={{-20,20},{0,40}})));
    Components.DoubleWishbonePushrod pushrod(steerable=false, lowerPivot(phi(start=-phi_sweep, fixed=true), w(start=2*phi_sweep/T_sweep, fixed=true)))
      "Internal spring: pushrod, rocker and a coil-over across the chassis"
      annotation (Placement(transformation(extent={{-20,-40},{0,-20}})));
    MB.Parts.FixedTranslation rigOffset(r={-1.2, 0, 0}, animation=false)
      "Places the pushrod corner behind the coil-over corner in the animation"
      annotation (Placement(transformation(extent={{-60,-40},{-40,-20}})));
    Modelica.Mechanics.Rotational.Sources.ConstantSpeed sweepA(w_fixed=2*phi_sweep/T_sweep, phi(start=-phi_sweep, fixed=true))
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,-10})));
    Modelica.Mechanics.Rotational.Sources.ConstantSpeed sweepB(w_fixed=2*phi_sweep/T_sweep, phi(start=-phi_sweep, fixed=true))
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,-80})));
    Modelica.Mechanics.Rotational.Components.SpringDamper rigA(c=2e5, d=2e3) "Stiff rig coupling"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,10})));
    Modelica.Mechanics.Rotational.Components.SpringDamper rigB(c=2e5, d=2e3) "Stiff rig coupling"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={-10,-55})));
    Real MR_coilOver = -der(coilOver.L_spring)/der(coilOver.travel) "Motion ratio: spring compression per unit wheel travel";
    Real MR_pushrod = -der(pushrod.L_spring)/der(pushrod.travel) "Motion ratio: spring compression per unit wheel travel";
    Real k_wheel_coilOver(unit="N/m") = coilOver.c_spring*MR_coilOver^2 "Wheel rate";
    Real k_wheel_pushrod(unit="N/m") = pushrod.c_spring*MR_pushrod^2 "Wheel rate";
  equation
    connect(world.frame_b, coilOver.frame_a);
    connect(world.frame_b, rigOffset.frame_a);
    connect(rigOffset.frame_b, pushrod.frame_a);
    connect(sweepA.flange, rigA.flange_a);
    connect(rigA.flange_b, coilOver.flange_arm);
    connect(sweepB.flange, rigB.flange_a);
    connect(rigB.flange_b, pushrod.flange_arm);
    annotation (experiment(StopTime=4, Interval=0.004), Documentation(info="<html>
<p>The two ways of springing the same double wishbone, swept from rebound to bump on a kinematic
rig (body fixed, lower arm turned slowly). On the left the coil-over is outboard, between the lower
arm and the body. Behind it the spring is inboard: a pushrod from the lower arm turns a bell-crank
rocker, which compresses a coil-over lying across the chassis, as on most single-seaters.</p>
<p>Plot <code>MR_coilOver</code> and <code>MR_pushrod</code> against the wheel travel. The rocker
geometry sets the motion ratio and how it changes with travel (rising rate), which the outboard
layout cannot. <code>k_wheel_*</code> is the resulting wheel rate.</p>
</html>"));
  end PushrodVsCoilOver;

  model MultilinkRig "Elasto-kinematic five-link corner on a wheel-pad rig: +-70 mm bounce"
    extends Modelica.Icons.Example;
    parameter SI.Length amplitude=0.07 "Pad bounce amplitude";
    parameter SI.Frequency f=0.4 "Bounce frequency";
    final parameter SI.Height z_chassis=pad.h0 + tyre.R0 - 3100/tyre.c_z "Chassis (wheel-centre) height, about the static ride height";
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    MB.Parts.FixedTranslation chassis(r={0, 0, z_chassis}, animation=false) "The body is held fixed"
      annotation (Placement(transformation(extent={{-60,30},{-40,50}})));
    Components.Multilink corner(animateWheel=false)
      annotation (Placement(transformation(extent={{-20,30},{0,50}})));
    Components.Tyre tyre(usePad=true, animation=true)
      annotation (Placement(transformation(extent={{20,10},{40,30}})));
    Components.WheelPad pad(r_pad={0, corner.r_WC[2]})
      annotation (Placement(transformation(extent={{0,-40},{20,-20}})));
    Modelica.Blocks.Sources.RealExpression demand(y=if time > 1 then amplitude*sin(2*Modelica.Constants.pi*f*(time - 1)) else 0)
      "Settle for 1 s, then bounce"
      annotation (Placement(transformation(extent={{-40,-40},{-20,-20}})));
    Modelica.Blocks.Sources.Constant noDrive(k=0)
      annotation (Placement(transformation(extent={{-10,0},{10,20}})));
    SI.Length travel = corner.travel "Wheel travel";
    Modelica.Units.NonSI.Angle_deg camber = corner.camber;
    Modelica.Units.NonSI.Angle_deg toe_in = corner.toe_in;
    SI.Length track_change = corner.track_change;
    SI.Force Fz = tyre.Fz "Pad load";
  equation
    connect(world.frame_b, chassis.frame_a);
    connect(chassis.frame_b, corner.frame_a);
    connect(corner.frame_wheel, tyre.frame_a);
    connect(noDrive.y, tyre.F_drive);
    connect(demand.y, pad.z_ref);
    connect(pad.z, tyre.z_pad);
    connect(pad.v, tyre.v_pad);
    annotation (experiment(StopTime=6, Interval=0.004), Documentation(info="<html>
<p>A rear five-link corner on a single-post rig, the way a suspension is measured on a K&amp;C
machine: the body is fixed and a hydraulic pad under the tyre moves the wheel. After 1 s of settling
the pad bounces +-70 mm at 0.4 Hz. Plot against <code>travel</code>:</p>
<ul>
<li><code>camber</code>: the short upper links make the wheel gain negative camber in bump.</li>
<li><code>toe_in</code>: bump steer, set by the height of the toe link relative to the lower links.</li>
<li><code>corner.F_link</code>: the forces in the five links, and <code>corner.link[i].deflection</code>,
    how much the bushes give.</li>
</ul>
</html>"));
  end MultilinkRig;

  model KCRig "Front axle on a K&C rig: pushrod double wishbones, anti-roll bar and steering; bounce, roll, then steer"
    extends Modelica.Icons.Example;
    parameter SI.Length bounce=0.05 "Bounce amplitude";
    parameter SI.Length rollAmp=0.035 "Roll amplitude at each pad (opposite sides)";
    parameter SI.Angle steerAmp=0.12 "Road-wheel steer amplitude";
    parameter SI.Force W=1300*Modelica.Constants.g_n*1.4/2.6/2 "Sprung load per front wheel";
    parameter Real MR=0.82 "Pushrod motion ratio at design (from PushrodVsCoilOver)";
    final parameter SI.Mass m_unsprung=left.m_wheel + left.m_upright + (left.m_lowerArm + left.m_upperArm)/2;
    final parameter SI.Height z_chassis=padL.h0 + tL.R0 - (W + m_unsprung*Modelica.Constants.g_n)/tL.c_z "Wheel-centre height";
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    MB.Parts.FixedTranslation chassis(r={0, 0, z_chassis}, width=0.06, animation=false) "The body is held fixed"
      annotation (Placement(transformation(extent={{-60,30},{-40,50}})));
    Components.DoubleWishbonePushrod left(side=1, animateWheel=false,
      F_spring_design=(W + (left.m_lowerArm + left.m_upperArm)/2*Modelica.Constants.g_n)/MR,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-20,40},{0,60}})));
    Components.DoubleWishbonePushrod right(side=-1, animateWheel=false,
      F_spring_design=(W + (right.m_lowerArm + right.m_upperArm)/2*Modelica.Constants.g_n)/MR,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-20,0},{0,20}})));
    Components.AntiRollBar arb(c=4000)
      annotation (Placement(transformation(extent={{-20,-30},{0,-10}})));
    Components.Tyre tL(usePad=true, animation=true, side=1)
      annotation (Placement(transformation(extent={{20,40},{40,60}})));
    Components.Tyre tR(usePad=true, animation=true, side=-1)
      annotation (Placement(transformation(extent={{20,0},{40,20}})));
    Components.WheelPad padL(r_pad={0, left.r_WC[2]})
      annotation (Placement(transformation(extent={{60,40},{80,60}})));
    Components.WheelPad padR(r_pad={0, -right.r_WC[2]})
      annotation (Placement(transformation(extent={{60,0},{80,20}})));
    Modelica.Blocks.Sources.RealExpression demandL(y=bounce*window(time, 1, 4)*sin(2*Modelica.Constants.pi*0.25*(time - 1))
      + rollAmp*window(time, 5, 4)*sin(2*Modelica.Constants.pi*0.25*(time - 5)))
      annotation (Placement(transformation(extent={{100,40},{80,60}})));
    Modelica.Blocks.Sources.RealExpression demandR(y=bounce*window(time, 1, 4)*sin(2*Modelica.Constants.pi*0.25*(time - 1))
      - rollAmp*window(time, 5, 4)*sin(2*Modelica.Constants.pi*0.25*(time - 5)))
      annotation (Placement(transformation(extent={{100,0},{80,20}})));
    Modelica.Blocks.Sources.RealExpression steer(y=steerAmp*window(time, 9, 4)*sin(2*Modelica.Constants.pi*0.25*(time - 9)))
      annotation (Placement(transformation(extent={{-60,70},{-40,90}})));
    Modelica.Blocks.Sources.Constant noDrive(k=0)
      annotation (Placement(transformation(extent={{-10,-60},{10,-40}})));
    SI.Length travel_left = left.travel;
    SI.Length travel_right = right.travel;
    Modelica.Units.NonSI.Angle_deg camber_left = left.camber;
    Modelica.Units.NonSI.Angle_deg camber_right = right.camber;
    SI.Force Fz_left = tL.Fz "Left pad load";
    SI.Force Fz_right = tR.Fz "Right pad load";
    SI.Torque arbTorque = arb.tau "Anti-roll bar torque";
  protected
    function window "1 inside [t0, t0 + T), 0 outside"
      input Real t;
      input Real t0;
      input Real T;
      output Real y;
    algorithm
      y := if t >= t0 and t < t0 + T then 1 else 0;
    end window;
  equation
    connect(world.frame_b, chassis.frame_a);
    connect(chassis.frame_b, left.frame_a);
    connect(chassis.frame_b, right.frame_a);
    connect(left.frame_wheel, tL.frame_a);
    connect(right.frame_wheel, tR.frame_a);
    connect(left.flange_arm, arb.flange_left);
    connect(right.flange_arm, arb.flange_right);
    connect(steer.y, left.steer);
    connect(steer.y, right.steer);
    connect(noDrive.y, tL.F_drive);
    connect(noDrive.y, tR.F_drive);
    connect(demandL.y, padL.z_ref);
    connect(demandR.y, padR.z_ref);
    connect(padL.z, tL.z_pad);
    connect(padL.v, tL.v_pad);
    connect(padR.z, tR.z_pad);
    connect(padR.v, tR.v_pad);
    annotation (experiment(StopTime=13, Interval=0.005), Documentation(info="<html>
<p>A kinematics and compliance (K&amp;C) test of a front axle, as on a K&amp;C machine: the body
is held, two wheel pads move the tyres, and the steering is turned. The axle has pushrod double
wishbones with inboard coil-overs and an anti-roll bar.</p>
<ul>
<li>0 to 1 s: settle on the pads.</li>
<li>1 to 5 s, bounce: both pads move together, +-50 mm. The anti-roll bar does not twist
    (<code>arbTorque</code> stays near zero); plot <code>camber_left</code> against <code>travel_left</code>.</li>
<li>5 to 9 s, roll: the pads move in opposite directions, +-35 mm. The bar twists and stiffens the
    axle in roll: <code>Fz_left - Fz_right</code> against <code>travel_left - travel_right</code>
    is the roll stiffness.</li>
<li>9 to 13 s, steer: the wheels steer +-7 deg about their kingpin axes (<code>left.kingpin.phi</code>);
    the kingpin inclination tilts the wheel a little as it steers (<code>camber_left</code>).</li>
</ul>
</html>"));
  end KCRig;

  model SevenPostRig "Full car on a seven-post rig: four wheel pads and three aero pull-down actuators"
    extends Modelica.Icons.Example;
    parameter SI.Length A=0.012 "Pad amplitude";
    parameter SI.Frequency f=1.5 "Excitation frequency";
    parameter SI.Force F_aeroFront=1500 "Front downforce";
    parameter SI.Force F_aeroRear=900 "Downforce on each rear loader";
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    Components.Vehicle car(onRig=true, z_ground=0.5, v_start=0, v_set=0)
      annotation (Placement(transformation(extent={{-20,-20},{20,20}})));
    Components.WheelPad pad[4](r_pad={{0, 0.75}, {0, -0.75}, {-car.wheelbase, 0.75}, {-car.wheelbase, -0.75}}, each h0=0.5)
      "FL, FR, RL, RR"
      annotation (Placement(transformation(extent={{-60,-50},{-40,-30}})));
    Modelica.Blocks.Sources.RealExpression demand[4](y={A*(heave + pitch + roll + warp), A*(heave + pitch - roll - warp),
      A*(heave - pitch + roll - warp), A*(heave - pitch - roll + warp)})
      annotation (Placement(transformation(extent={{-100,-50},{-80,-30}})));
    Modelica.Blocks.Sources.Constant straight(k=0)
      annotation (Placement(transformation(extent={{-60,-10},{-40,10}})));
    MB.Parts.FixedTranslation toFrontLoader(r={0.3, 0, -0.22}, animation=false)
      annotation (Placement(transformation(extent={{30,20},{50,40}})));
    MB.Parts.FixedTranslation toRearLeftLoader(r={-2.3, 0.35, -0.22}, animation=false)
      annotation (Placement(transformation(extent={{30,-10},{50,10}})));
    MB.Parts.FixedTranslation toRearRightLoader(r={-2.3, -0.35, -0.22}, animation=false)
      annotation (Placement(transformation(extent={{30,-40},{50,-20}})));
    Components.AeroLoader frontLoader
      annotation (Placement(transformation(extent={{80,20},{60,40}})));
    Components.AeroLoader rearLeftLoader
      annotation (Placement(transformation(extent={{80,-10},{60,10}})));
    Components.AeroLoader rearRightLoader
      annotation (Placement(transformation(extent={{80,-40},{60,-20}})));
    Modelica.Blocks.Sources.Ramp aeroFront(height=F_aeroFront, duration=0.6, startTime=0.3)
      annotation (Placement(transformation(extent={{120,20},{100,40}})));
    Modelica.Blocks.Sources.Ramp aeroRear(height=F_aeroRear, duration=0.6, startTime=0.3)
      annotation (Placement(transformation(extent={{120,-20},{100,0}})));
    Real heave = burst(time, 1.5, f) "Heave: all four pads together";
    Real pitch = burst(time, 4, f) "Pitch: front against rear";
    Real roll = burst(time, 6.5, f) "Roll: left against right";
    Real warp = burst(time, 9, f) "Warp: diagonals against each other";
    Modelica.Units.NonSI.Angle_deg roll_body = car.roll;
    Modelica.Units.NonSI.Angle_deg pitch_body = car.pitch;
    SI.Position heave_body = car.heave;
    SI.Force Fz[4] = {car.tFL.Fz, car.tFR.Fz, car.tRL.Fz, car.tRR.Fz} "Tyre loads FL, FR, RL, RR";
  protected
    function burst "Sine burst of 3 periods with a smooth envelope, starting at t0"
      input Real t;
      input Real t0;
      input Real f;
      output Real y;
    protected
      Real T = 3/f;
    algorithm
      y := if t > t0 and t < t0 + T then sin(Modelica.Constants.pi*(t - t0)/T)^2*sin(2*Modelica.Constants.pi*f*(t - t0)) else 0;
    end burst;
  equation
    connect(straight.y, car.delta);
    connect(demand.y, pad.z_ref);
    connect(pad[1].z, car.z_pad[1]);
    connect(pad[2].z, car.z_pad[2]);
    connect(pad[3].z, car.z_pad[3]);
    connect(pad[4].z, car.z_pad[4]);
    connect(pad[1].v, car.v_pad[1]);
    connect(pad[2].v, car.v_pad[2]);
    connect(pad[3].v, car.v_pad[3]);
    connect(pad[4].v, car.v_pad[4]);
    connect(car.frame_body, toFrontLoader.frame_a);
    connect(car.frame_body, toRearLeftLoader.frame_a);
    connect(car.frame_body, toRearRightLoader.frame_a);
    connect(toFrontLoader.frame_b, frontLoader.frame_b);
    connect(toRearLeftLoader.frame_b, rearLeftLoader.frame_b);
    connect(toRearRightLoader.frame_b, rearRightLoader.frame_b);
    connect(aeroFront.y, frontLoader.F);
    connect(aeroRear.y, rearLeftLoader.F);
    connect(aeroRear.y, rearRightLoader.F);
    annotation (experiment(StopTime=11.5, Interval=0.005), Documentation(info="<html>
<p>The full car on a seven-post rig, the standard tool for tuning race-car dampers. Each tyre
stands on a hydraulic wheel pad (four posts) and three pull-down actuators (the other three posts)
load the body with the downforce: 1500 N at the front and 900 N on each rear loader, ramped in
during the first second.</p>
<p>The pads then play four 1.5 Hz sine bursts of 12 mm, one per body mode:
heave (all pads together, from 1.5 s), pitch (front against rear, from 4 s), roll (left against
right, from 6.5 s) and warp (one diagonal against the other, from 9 s). Plot
<code>heave_body</code>, <code>pitch_body</code>, <code>roll_body</code> and the tyre loads
<code>Fz</code>; the warp burst barely moves the body but loads one diagonal of tyres against the
other, which is what the anti-roll bars resist.</p>
</html>"));
  end SevenPostRig;

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
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
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
and <code>F_tyre</code> with <code>F_tyre1D</code>. Peak travel agrees to 0.4 mm and peak tyre load
to about 1.5 %; what remains is the geometry the 1-D model leaves out, such as the lateral tyre scrub
from track change and the moving arm inertias.</p>
</html>"));
  end QuarterCarBump;

  model StepSteer "Full car: 2 deg front-wheel step steer at 72 km/h"
    extends Modelica.Icons.Example;
    parameter SI.Angle delta_step=Modelica.Units.Conversions.from_deg(2) "Road-wheel steer angle";
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
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
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
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
    inner MB.World world(n={0,0,-1}, animateWorld=false, animateGravity=false)
      annotation (Placement(transformation(extent={{-90,30},{-70,50}})));
    Components.Vehicle car(v_start=10, bump_h=0.05, bump_L=0.8, bump_x0=10, bump_yMin=0, bump_yMax=5)
      annotation (Placement(transformation(extent={{0,-20},{40,20}})));
    Modelica.Blocks.Sources.Constant straight(k=0)
      annotation (Placement(transformation(extent={{-60,-10},{-40,10}})));
    MB.Visualizers.FixedShape bump(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/bump.stl",
      r_shape={car.bump_x0, car.bump_yMin, 0}, lengthDirection={1,0,0}, widthDirection={0,1,0},
      length=car.bump_L, width=car.bump_yMax - car.bump_yMin, height=car.bump_h, extra=1, color={235,185,0})
      "The bump: a unit half-cosine mesh scaled to the bump size (animation only)"
      annotation (Placement(transformation(extent={{-60,30},{-40,50}})));
  equation
    connect(straight.y, car.delta);
    connect(world.frame_b, bump.frame_a);
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
