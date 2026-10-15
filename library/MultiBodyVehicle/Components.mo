within MultiBodyVehicle;
package Components "Suspension corners, tyre and vehicle"
  extends Modelica.Icons.Package;
  import SI = Modelica.Units.SI;
  import MB = Modelica.Mechanics.MultiBody;
  import Modelica.Units.Conversions.to_deg;

  function mirror "Mirror a left-hand hardpoint to the side given by side (+1 left, -1 right)"
    extends Modelica.Icons.Function;
    input SI.Position r[3];
    input Real side;
    output SI.Position y[3];
  algorithm
    y := {r[1], side*r[2], r[3]};
  end mirror;

  function roadHeight "Flat road with an optional half-cosine bump across a lateral band"
    extends Modelica.Icons.Function;
    input SI.Position x;
    input SI.Position y;
    input SI.Height h "Bump height (0: flat road)";
    input SI.Length L "Bump length";
    input SI.Position x0 "Bump start";
    input SI.Position yMin "Bump covers yMin <= y <= yMax";
    input SI.Position yMax;
    output SI.Height z;
  algorithm
    z := if x < x0 or x > x0 + L or y < yMin or y > yMax then 0
         else h/2*(1 - cos(2*Modelica.Constants.pi*(x - x0)/L));
  end roadHeight;

  partial model PartialCorner "Chassis frame, wheel-centre frame and corner kinematics outputs"
    parameter Real side=1 "+1 left-hand corner, -1 right-hand (mirrored in y)";
    MB.Interfaces.Frame_a frame_a "Chassis reference (axle centre line, wheel-centre height)"
      annotation (Placement(transformation(extent={{-116,-16},{-84,16}})));
    MB.Interfaces.Frame_b frame_wheel "Wheel centre (steers and moves with the upright, does not spin)"
      annotation (Placement(transformation(extent={{84,-16},{116,16}})));
    Modelica.Mechanics.Rotational.Interfaces.Flange_a flange_arm
      "Lower-arm pivot; drive it for a kinematic sweep, leave it open in a vehicle"
      annotation (Placement(transformation(extent={{-10,-110},{10,-90}})));
    SI.Position r_wheel[3] = MB.Frames.resolve2(frame_a.R, frame_wheel.r_0 - frame_a.r_0)
      "Wheel centre relative to the chassis frame, in chassis axes";
    SI.Length travel "Wheel travel from design position (bump positive)";
    SI.Length track_change "Lateral wheel-centre movement from design (outboard positive)";
    Modelica.Units.NonSI.Angle_deg camber "Camber relative to the body (top of wheel outboard positive)";
    Modelica.Units.NonSI.Angle_deg toe_in "Toe relative to the body (toe-in positive)";
  protected
    parameter SI.Position r_wheel_design[3] "Wheel centre at design, left-hand coordinates";
    Real n_spin[3] = MB.Frames.resolve2(frame_a.R, MB.Frames.resolve1(frame_wheel.R, {0, side, 0}))
      "Outboard spin axis in chassis axes";
    Real e_head[3] = MB.Frames.resolve2(frame_a.R, MB.Frames.resolve1(frame_wheel.R, {1, 0, 0}))
      "Wheel heading in chassis axes";
  equation
    travel = r_wheel[3] - r_wheel_design[3];
    track_change = side*r_wheel[2] - r_wheel_design[2];
    camber = -to_deg(asin(n_spin[3]));
    toe_in = -side*to_deg(atan2(e_head[2], e_head[1]));
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}})));
  end PartialCorner;

  model DoubleWishbone "Double-wishbone corner with coil-over on the lower arm and a steerable upright"
    extends PartialCorner(r_wheel_design=r_WC);
    parameter SI.Position r_LI[3]={0, 0.25, -0.12} "Lower arm inner pivot" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_LB[3]={0, 0.70, -0.12} "Lower ball joint" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_UI[3]={0, 0.40, 0.16} "Upper arm inner pivot" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_UB[3]={0, 0.66, 0.19} "Upper ball joint" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_WC[3]={0, 0.75, 0} "Wheel centre" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SL[3]={0, 0.55, -0.12} "Spring seat on the lower arm" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SC[3]={0, 0.50, 0.30} "Spring top mount on the body" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.TranslationalSpringConstant c_spring=55e3 "Coil spring rate" annotation (Dialog(group="Spring and damper"));
    parameter SI.Force F_spring_design=4800 "Spring force at the design position" annotation (Dialog(group="Spring and damper"));
    parameter SI.TranslationalDampingConstant d_damper=4000 "Damper rate along the coil-over" annotation (Dialog(group="Spring and damper"));
    parameter SI.Mass m_lowerArm=6;
    parameter SI.Mass m_upperArm=3;
    parameter SI.Mass m_upright=8 "Upright, hub and brake";
    parameter SI.Mass m_wheel=22 "Wheel and tyre";
    parameter SI.Inertia I_wheel[3]={0.8, 1.4, 0.8} "Wheel inertia about the wheel centre (x, spin, z)";
    parameter Boolean steerable=true "false: steering input is ignored and the upright is locked";
    final parameter SI.Length L_spring_design=Modelica.Math.Vectors.length(r_SC - r_SL);
    SI.Length L_spring = coilOver.length "Coil-over length";
    SI.Force F_spring = coilOver.f "Coil-over force (tension positive)";
    Modelica.Blocks.Interfaces.RealInput steer(unit="rad") if steerable "Road-wheel steer angle (positive left)"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,110})));

    MB.Parts.FixedTranslation toLI(r=mirror(r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-80,-50},{-60,-30}})));
    MB.Parts.FixedTranslation toUI(r=mirror(r_UI, side), animation=false)
      annotation (Placement(transformation(extent={{-80,30},{-60,50}})));
    MB.Parts.FixedTranslation toSC(r=mirror(r_SC, side), animation=false)
      annotation (Placement(transformation(extent={{-80,70},{-60,90}})));
    MB.Joints.Revolute lowerPivot(n={1,0,0}, useAxisFlange=true, stateSelect=StateSelect.always,
      phi(start=0), w(start=0)) "Lower arm inner pivot (chassis)"
      annotation (Placement(transformation(extent={{-50,-50},{-30,-30}})));
    MB.Parts.BodyShape lowerArm(r=mirror(r_LB - r_LI, side), r_CM=mirror(r_LB - r_LI, side)/2, m=m_lowerArm, animation=false)
      annotation (Placement(transformation(extent={{-20,-50},{0,-30}})));
    MB.Parts.FixedTranslation toSL(r=mirror(r_SL - r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-20,-80},{0,-60}})));
    MB.Joints.Assemblies.JointRRR links(n_a={1,0,0}, rRod1_ia=mirror(r_UB - r_LB, side), rRod2_ib=mirror(r_UB - r_UI, side), animation=false)
      "Upright (rod 1) and upper arm (rod 2): closes the planar loop analytically, no constraint equations"
      annotation (Placement(transformation(extent={{-20,-20},{20,20}}, rotation=90, origin={20,0})));
    MB.Parts.Body upright(m=m_upright, r_CM=mirror(r_WC - r_LB, side)*0.6, animation=false) "Upright, hub and brake"
      annotation (Placement(transformation(extent={{50,-10},{70,10}})));
    MB.Parts.Body upperArm(m=m_upperArm, r_CM=mirror(r_UB - r_UI, side)/2, animation=false)
      annotation (Placement(transformation(extent={{-20,50},{-40,70}})));
    MB.Joints.Revolute kingpin(n=Modelica.Math.Vectors.normalize(mirror(r_UB - r_LB, side)), useAxisFlange=true)
      "Steering axis through both ball joints"
      annotation (Placement(transformation(extent={{50,-50},{70,-30}})));
    Modelica.Mechanics.Rotational.Sources.Position steering(exact=false, f_crit=20) if steerable
      annotation (Placement(transformation(extent={{30,70},{50,90}})));
    Modelica.Mechanics.Rotational.Components.Fixed steeringLock if not steerable
      annotation (Placement(transformation(extent={{70,-80},{90,-60}})));
    MB.Parts.FixedTranslation spindle(r=mirror(r_WC - r_LB, side), animation=false)
      annotation (Placement(transformation(extent={{76,-50},{96,-30}})));
    MB.Parts.Body wheel(m=m_wheel, r_CM={0,0,0}, I_11=I_wheel[1], I_22=I_wheel[2], I_33=I_wheel[3], animation=false)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={86,-70})));
    MB.Forces.SpringDamperParallel coilOver(c=c_spring, d=d_damper,
      s_unstretched=L_spring_design + F_spring_design/c_spring, animation=false)
      "Coil spring and damper between lower arm and body"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={20,-80})));
  equation
    connect(frame_a, toLI.frame_a);
    connect(frame_a, toUI.frame_a);
    connect(frame_a, toSC.frame_a);
    connect(toLI.frame_b, lowerPivot.frame_a);
    connect(lowerPivot.frame_b, lowerArm.frame_a);
    connect(lowerPivot.frame_b, toSL.frame_a);
    connect(lowerArm.frame_b, links.frame_a);
    connect(links.frame_b, toUI.frame_b);
    connect(links.frame_ia, upright.frame_a);
    connect(links.frame_ib, upperArm.frame_a);
    connect(links.frame_ia, kingpin.frame_a);
    connect(kingpin.frame_b, spindle.frame_a);
    connect(spindle.frame_b, wheel.frame_a);
    connect(spindle.frame_b, frame_wheel);
    connect(toSL.frame_b, coilOver.frame_a);
    connect(coilOver.frame_b, toSC.frame_b);
    connect(lowerPivot.axis, flange_arm);
    connect(steer, steering.phi_ref);
    connect(steering.flange, kingpin.axis);
    connect(steeringLock.flange, kingpin.axis);
    annotation (Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Line(points={{-80,-40},{20,-55}}, color={0,0,0}, thickness=1),
        Line(points={{-70,40},{10,35}}, color={0,0,0}, thickness=1),
        Line(points={{20,-55},{10,35}}, color={0,0,0}, thickness=1),
        Ellipse(extent={{-86,-34},{-74,-46}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-76,46},{-64,34}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{30,70},{70,-70}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid, radius=8),
        Line(points={{-40,-46},{-50,-30},{-30,-20},{-50,-10},{-30,0},{-50,10},{-40,20},{-40,70}}, color={0,0,255}),
        Line(points={{-100,0},{-80,0},{-80,90}}, color={95,95,95}),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>Short-long-arm (SLA) double wishbone in the front view. Lower arm, upright and upper arm
form a planar four-bar with the body. The lower arm pivot is the joint with states; upright and
upper arm are the two rods of a <code>JointRRR</code>, which solves the loop analytically, so the
corner adds no constraint equations and no non-linear loop. The upper arm is
shorter than the lower arm, which gives negative camber gain in bump.</p>
<p>The coil-over acts between a seat on the lower arm and a mount on the body. Its free length is
set so that it carries <code>F_spring_design</code> at the design position. The steering joint turns
the spindle about the kingpin axis through the two ball joints (no tie rod: steering is ideal).</p>
</html>"));
  end DoubleWishbone;

  model MacPherson "MacPherson strut corner: telescopic strut with coil-over, lower control arm"
    extends PartialCorner(r_wheel_design=r_WC);
    parameter SI.Position r_TM[3]={0, 0.58, 0.60} "Strut top mount" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SB[3]={0, 0.66, 0.08} "Strut-to-upright clamp" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_LI[3]={0, 0.28, -0.12} "Lower arm inner pivot" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_LB[3]={0, 0.71, -0.14} "Lower ball joint" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_WC[3]={0, 0.76, 0} "Wheel centre" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.TranslationalSpringConstant c_spring=30e3 "Coil spring rate" annotation (Dialog(group="Spring and damper"));
    parameter SI.Force F_spring_design=3500 "Spring force at the design position" annotation (Dialog(group="Spring and damper"));
    parameter SI.TranslationalDampingConstant d_damper=2500 "Strut damping" annotation (Dialog(group="Spring and damper"));
    parameter SI.Mass m_lowerArm=5;
    parameter SI.Mass m_strut=4 "Upper strut and spring";
    parameter SI.Mass m_upright=10 "Upright, damper tube, hub and brake";
    parameter SI.Mass m_wheel=22 "Wheel and tyre";
    parameter SI.Inertia I_wheel[3]={0.8, 1.4, 0.8};
    final parameter SI.Length L_strut_design=Modelica.Math.Vectors.length(r_SB - r_TM);
    SI.Length L_strut = strut.s "Strut length, top mount to clamp";
    SI.Force F_spring = coilOver.f "Coil-over force (compression negative)";

    MB.Parts.FixedTranslation toTM(r=mirror(r_TM, side), animation=false)
      annotation (Placement(transformation(extent={{-80,50},{-60,70}})));
    MB.Parts.FixedTranslation toLI(r=mirror(r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-80,-50},{-60,-30}})));
    MB.Joints.Revolute topMount(n={1,0,0}) "Strut top mount (front-view hinge)"
      annotation (Placement(transformation(extent={{-50,50},{-30,70}})));
    MB.Parts.Body strutBody(m=m_strut, r_CM={0,0,0}, animation=false)
      annotation (Placement(transformation(extent={{-30,80},{-10,100}})));
    MB.Joints.Prismatic strut(n=Modelica.Math.Vectors.normalize(mirror(r_SB - r_TM, side)), useAxisFlange=true,
      s(start=L_strut_design)) "Telescopic damper"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,40})));
    Modelica.Mechanics.Translational.Components.SpringDamper coilOver(c=c_spring, d=d_damper,
      s_rel0=L_strut_design + F_spring_design/c_spring) "Coil spring and damper along the strut"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={30,40})));
    MB.Parts.BodyShape upright(r=mirror(r_LB - r_SB, side), r_CM=mirror(r_WC - r_SB, side)*0.7, m=m_upright, animation=false)
      "From the strut clamp down to the lower ball joint"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,0})));
    MB.Joints.RevolutePlanarLoopConstraint lowerBall(n={1,0,0}) "Lower ball joint; closes the planar loop"
      annotation (Placement(transformation(extent={{10,-50},{-10,-30}})));
    MB.Parts.BodyShape lowerArm(r=mirror(r_LB - r_LI, side), r_CM=mirror(r_LB - r_LI, side)/2, m=m_lowerArm, animation=false)
      annotation (Placement(transformation(extent={{-30,-50},{-10,-30}})));
    MB.Joints.Revolute lowerPivot(n={1,0,0}, useAxisFlange=true, stateSelect=StateSelect.always,
      phi(start=0), w(start=0)) "Lower arm inner pivot (chassis)"
      annotation (Placement(transformation(extent={{-56,-50},{-36,-30}})));
    MB.Parts.FixedTranslation spindle(r=mirror(r_WC - r_SB, side), animation=false)
      annotation (Placement(transformation(extent={{40,-10},{60,10}})));
    MB.Parts.Body wheel(m=m_wheel, r_CM={0,0,0}, I_11=I_wheel[1], I_22=I_wheel[2], I_33=I_wheel[3], animation=false)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={70,-30})));
  equation
    connect(frame_a, toTM.frame_a);
    connect(frame_a, toLI.frame_a);
    connect(toTM.frame_b, topMount.frame_a);
    connect(topMount.frame_b, strutBody.frame_a);
    connect(topMount.frame_b, strut.frame_a);
    connect(strut.support, coilOver.flange_a);
    connect(coilOver.flange_b, strut.axis);
    connect(strut.frame_b, upright.frame_a);
    connect(upright.frame_b, lowerBall.frame_a);
    connect(lowerBall.frame_b, lowerArm.frame_b);
    connect(lowerArm.frame_a, lowerPivot.frame_b);
    connect(lowerPivot.frame_a, toLI.frame_b);
    connect(strut.frame_b, spindle.frame_a);
    connect(spindle.frame_b, wheel.frame_a);
    connect(spindle.frame_b, frame_wheel);
    connect(lowerPivot.axis, flange_arm);
    annotation (Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Line(points={{-80,-50},{20,-60}}, color={0,0,0}, thickness=1),
        Line(points={{-10,80},{20,-60}}, color={0,0,0}, thickness=1),
        Rectangle(extent={{-6,40},{14,-20}}, lineColor={0,0,0}, fillColor={160,160,160}, fillPattern=FillPattern.Solid, rotation=-12, origin={2,0}),
        Line(points={{-16,70},{4,64},{-12,56},{8,50},{-8,42},{12,36}}, color={0,0,255}),
        Ellipse(extent={{-86,-44},{-74,-56}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{30,70},{70,-70}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid, radius=8),
        Line(points={{-100,0},{-80,0},{-80,90},{-10,90}}, color={95,95,95}),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>MacPherson strut in the front view. The strut is a prismatic joint hinged at the top mount,
the upright is clamped to its lower tube, and a lower control arm locates the bottom of the
upright. The lower ball joint is a <code>RevolutePlanarLoopConstraint</code> that closes the loop.
The coil spring and damper act along the strut.</p>
</html>"));
  end MacPherson;

  model Tyre "Point-contact tyre: vertical spring-damper with lift-off, Pacejka-type lateral force, rolling resistance"
    import Modelica.Math.Vectors.normalize;
    MB.Interfaces.Frame_a frame_a "Wheel centre (spin axis = y of this frame)"
      annotation (Placement(transformation(extent={{-16,84},{16,116}}, rotation=270)));
    Modelica.Blocks.Interfaces.RealInput F_drive(unit="N") "Longitudinal drive (+) or brake (-) force"
      annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    parameter SI.Radius R0=0.31 "Unloaded radius";
    parameter SI.TranslationalSpringConstant c_z=250e3 "Vertical stiffness";
    parameter SI.TranslationalDampingConstant d_z=300 "Vertical damping";
    parameter Real mu=1.0 "Peak friction coefficient";
    parameter Real B=12 "Pacejka stiffness factor (cornering stiffness = B*C*mu*Fz)";
    parameter Real C=1.3 "Pacejka shape factor";
    parameter Real f_rr=0.012 "Rolling resistance coefficient";
    parameter SI.Velocity v_eps=0.5 "Slip-angle regularisation speed";
    parameter SI.Velocity v_belt=0 "Road surface speed in -x (flat-belt rig); 0 on a fixed road";
    parameter SI.Height bump_h=0 "Bump height (0: flat road)" annotation (Dialog(group="Road"));
    parameter SI.Length bump_L=1 "Bump length" annotation (Dialog(group="Road"));
    parameter SI.Position bump_x0=10 "Bump start along x" annotation (Dialog(group="Road"));
    parameter SI.Position bump_yMin=-10 "Bump covers bump_yMin <= y <= bump_yMax" annotation (Dialog(group="Road"));
    parameter SI.Position bump_yMax=10 annotation (Dialog(group="Road"));

    SI.Position r[3] = frame_a.r_0 "Wheel centre";
    SI.Velocity v[3] = der(frame_a.r_0) "Wheel centre velocity";
    SI.Height z_road = roadHeight(r[1] + v_belt*time, r[2], bump_h, bump_L, bump_x0, bump_yMin, bump_yMax);
    SI.Length deflection = R0 - (r[3] - z_road) "Radial tyre deflection";
    SI.Velocity v_long "Contact speed along the wheel heading, relative to the road";
    SI.Velocity v_lat "Contact speed across the wheel, relative to the road";
    SI.Angle alpha "Slip angle";
    SI.Force Fz "Vertical load";
    SI.Force Fy "Lateral force (wheel axes)";
    SI.Force Fx "Longitudinal force (wheel axes)";
    SI.Force F[3] "Force on the wheel, world axes";
  protected
    Real e_x[3] = MB.Frames.resolve1(frame_a.R, {1, 0, 0});
    Real e_long[3] = normalize({e_x[1], e_x[2], 0});
    Real e_lat[3] = {-e_long[2], e_long[1], 0};
    SI.Position r_contact[3] = {0, 0, -(r[3] - z_road)} "Contact point relative to the wheel centre";
  equation
    v_long = (v + {v_belt, 0, 0})*e_long;
    v_lat = v*e_lat;
    alpha = atan(v_lat/(abs(v_long) + v_eps));
    Fz = if deflection > 0 then max(0, c_z*deflection - d_z*v[3]) else 0;
    Fy = -mu*Fz*sin(C*atan(B*alpha));
    Fx = F_drive - f_rr*Fz*tanh(v_long/v_eps);
    F = Fx*e_long + Fy*e_lat + {0, 0, Fz};
    frame_a.f = -MB.Frames.resolve2(frame_a.R, F);
    frame_a.t = -MB.Frames.resolve2(frame_a.R, cross(r_contact, F));
    annotation (Icon(graphics={
        Ellipse(extent={{-60,90},{60,-90}}, lineColor={0,0,0}, fillColor={50,50,50}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-30,45},{30,-45}}, lineColor={0,0,0}, fillColor={200,200,200}, fillPattern=FillPattern.Solid),
        Line(points={{-100,-95},{100,-95}}, color={95,95,95}, thickness=1),
        Text(extent={{-150,-100},{150,-140}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>The wheel does not spin: the frame follows the upright. The contact point is taken straight
below the wheel centre on the road surface, and the force is applied there, so it also loads the
suspension with the correct moment arm.</p>
<ul>
<li>Vertical: <code>Fz = max(0, c_z*deflection - d_z*dz/dt)</code>, zero when the tyre leaves the road.</li>
<li>Lateral: <code>Fy = -mu*Fz*sin(C*atan(B*alpha))</code>, with slip angle
    <code>alpha = atan(v_lat/(|v_long| + v_eps))</code>.</li>
<li>Longitudinal: the drive/brake input minus rolling resistance <code>f_rr*Fz</code>.</li>
</ul>
<p>The road is flat with an optional half-cosine bump between <code>bump_yMin</code> and
<code>bump_yMax</code>. With <code>v_belt</code> &gt; 0 the road moves under a stationary wheel,
as on a flat-belt test rig.</p>
</html>"));
  end Tyre;

  model AntiRollBar "Torsion anti-roll bar between the left and right lower-arm pivots"
    parameter SI.RotationalSpringConstant c "Torsional rate per radian of opposite arm rotation";
    Modelica.Mechanics.Rotational.Interfaces.Flange_a flange_left "Left lower-arm pivot"
      annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
    Modelica.Mechanics.Rotational.Interfaces.Flange_b flange_right "Right lower-arm pivot"
      annotation (Placement(transformation(extent={{90,-10},{110,10}})));
    SI.Angle twist = flange_left.phi + flange_right.phi
      "Bar twist; the mirrored arms turn in opposite senses in heave, so heave does not twist the bar";
    SI.Torque tau = c*twist "Bar torque";
  equation
    flange_left.tau = tau;
    flange_right.tau = tau;
    annotation (Icon(graphics={
        Line(points={{-90,0},{-60,0},{-60,-40},{60,-40},{60,0},{90,0}}, color={0,0,0}, thickness=1),
        Text(extent={{-150,60},{150,20}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>The left lower arm turns positively about x in bump and the mirrored right arm negatively, so
the sum of the two pivot angles is zero in pure heave and twice the arm angle in roll. Each arm sees
the joint torque <code>c*(phi_left + phi_right)</code>, reacted by the body.</p>
</html>"));
  end AntiRollBar;

  model Vehicle "Full car: 6-DOF sprung body on four double-wishbone corners with tyres"
    parameter SI.Mass m_body=1300 "Sprung mass";
    parameter SI.Length wheelbase=2.6;
    parameter SI.Length a_CG=1.2 "CG behind the front axle";
    parameter SI.Height h_CG=0.25 "CG above the wheel centres (about 0.55 m above ground)";
    parameter SI.Inertia I_roll=450;
    parameter SI.Inertia I_pitch=1900;
    parameter SI.Inertia I_yaw=2100;
    parameter SI.Velocity v_start=20 "Initial forward speed";
    parameter SI.Velocity v_set=v_start "Speed held by the rear-axle drive controller";
    parameter Real k_speed=3000 "Speed controller gain (N per m/s)";
    parameter SI.TranslationalSpringConstant c_front=55e3 "Front coil spring rate";
    parameter SI.TranslationalSpringConstant c_rear=50e3 "Rear coil spring rate";
    parameter SI.TranslationalDampingConstant d_front=4000;
    parameter SI.TranslationalDampingConstant d_rear=3600;
    parameter SI.RotationalSpringConstant c_arb_front=4000 "Front anti-roll bar rate (lower-arm angle)";
    parameter SI.RotationalSpringConstant c_arb_rear=1500 "Rear anti-roll bar rate (lower-arm angle)";
    parameter Real motionRatio=0.65 "Spring-to-wheel motion ratio at design (from SuspensionKinematics)";
    parameter SI.Height bump_h=0 "Road bump height" annotation (Dialog(group="Road"));
    parameter SI.Length bump_L=0.8 annotation (Dialog(group="Road"));
    parameter SI.Position bump_x0=20 annotation (Dialog(group="Road"));
    parameter SI.Position bump_yMin=0 "Bump only under the left wheels by default" annotation (Dialog(group="Road"));
    parameter SI.Position bump_yMax=5 annotation (Dialog(group="Road"));
    final parameter SI.Force W_front=m_body*Modelica.Constants.g_n*(wheelbase - a_CG)/wheelbase/2 "Sprung load per front wheel";
    final parameter SI.Force W_rear=m_body*Modelica.Constants.g_n*a_CG/wheelbase/2 "Sprung load per rear wheel";
    final parameter SI.Mass m_corner=FL.m_wheel + FL.m_upright + FL.m_lowerArm + FL.m_upperArm "Moving mass of one corner";
    final parameter SI.Force F_armShare=(FL.m_lowerArm + FL.m_upperArm)/2*Modelica.Constants.g_n
      "Arm weight carried by the spring";
    final parameter SI.Length dz_front=(W_front + m_corner*Modelica.Constants.g_n)/tFL.c_z "Static front tyre deflection";
    final parameter SI.Length dz_rear=(W_rear + m_corner*Modelica.Constants.g_n)/tRL.c_z "Static rear tyre deflection";
    final parameter SI.Height z_start=tFL.R0 - dz_front "Initial height of the front axle line (wheel centres at design)";
    final parameter SI.Angle pitch_start=atan((dz_front - dz_rear)/wheelbase) "Initial pitch so all four tyres start at static load";

    Modelica.Blocks.Interfaces.RealInput delta(unit="rad") "Front road-wheel steer angle (positive left)"
      annotation (Placement(transformation(extent={{-120,-10},{-100,10}})));

    MB.Parts.Body body(m=m_body, r_CM={-a_CG, 0, h_CG}, I_11=I_roll, I_22=I_pitch, I_33=I_yaw,
      enforceStates=true, useQuaternions=false, sequence_angleStates={3,2,1},
      r_0(start={0, 0, z_start}, each fixed=true), v_0(start={v_start, 0, 0}, each fixed=true),
      angles_fixed=true, angles_start={0, pitch_start, 0}, w_0_fixed=true, animation=false)
      "Sprung body; frame_a on the front axle centre line at wheel-centre height"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={0,50})));
    MB.Parts.FixedTranslation toRear(r={-wheelbase, 0, 0}, animation=false)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,-10})));
    DoubleWishbone FL(side=1, c_spring=c_front, d_damper=d_front, F_spring_design=(W_front + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-30,40},{-50,60}})));
    DoubleWishbone FR(side=-1, c_spring=c_front, d_damper=d_front, F_spring_design=(W_front + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{30,40},{50,60}})));
    DoubleWishbone RL(side=1, steerable=false, c_spring=c_rear, d_damper=d_rear, F_spring_design=(W_rear + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-30,-50},{-50,-30}})));
    DoubleWishbone RR(side=-1, steerable=false, c_spring=c_rear, d_damper=d_rear, F_spring_design=(W_rear + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{30,-50},{50,-30}})));
    Tyre tFL(bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{-70,20},{-90,40}})));
    Tyre tFR(bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{70,20},{90,40}})));
    Tyre tRL(bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{-70,-70},{-90,-50}})));
    Tyre tRR(bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{70,-70},{90,-50}})));
    Modelica.Blocks.Sources.Constant noDrive(k=0)
      annotation (Placement(transformation(extent={{-10,72},{10,92}})));
    AntiRollBar arbFront(c=c_arb_front)
      annotation (Placement(transformation(extent={{-10,20},{10,40}})));
    AntiRollBar arbRear(c=c_arb_rear)
      annotation (Placement(transformation(extent={{-10,-80},{10,-60}})));

    SI.Velocity v_x "Forward speed (body axes)";
    SI.Velocity v_y "Lateral speed (body axes)";
    SI.Acceleration a_y "Lateral acceleration at the body reference (body axes)";
    Real yawRate(unit="deg/s") "Yaw rate";
    Modelica.Units.NonSI.Angle_deg roll "Body roll angle (positive right side down)";
    Modelica.Units.NonSI.Angle_deg pitch "Body pitch angle (positive nose down)";
    Modelica.Units.NonSI.Angle_deg sideslip "Body sideslip angle at the reference point";
    SI.Position heave = body.frame_a.r_0[3] - z_start "Heave from the initial height";
    SI.Force F_drive "Rear-axle drive force from the speed controller";
  protected
    Real e_fwd[3] = MB.Frames.resolve1(body.frame_a.R, {1, 0, 0});
    Real e_left[3] = MB.Frames.resolve1(body.frame_a.R, {0, 1, 0});
    SI.AngularVelocity w_body[3] = MB.Frames.angularVelocity2(body.frame_a.R);
  equation
    v_x = body.v_0*e_fwd;
    v_y = body.v_0*e_left;
    a_y = body.a_0*e_left;
    yawRate = to_deg(w_body[3]);
    roll = to_deg(asin(e_left[3]));
    pitch = -to_deg(asin(e_fwd[3]));
    sideslip = to_deg(atan2(v_y, v_x));
    F_drive = k_speed*(v_set - v_x);
    tRL.F_drive = F_drive/2;
    tRR.F_drive = F_drive/2;
    connect(body.frame_a, FL.frame_a);
    connect(body.frame_a, FR.frame_a);
    connect(body.frame_a, toRear.frame_a);
    connect(toRear.frame_b, RL.frame_a);
    connect(toRear.frame_b, RR.frame_a);
    connect(FL.frame_wheel, tFL.frame_a);
    connect(FR.frame_wheel, tFR.frame_a);
    connect(RL.frame_wheel, tRL.frame_a);
    connect(RR.frame_wheel, tRR.frame_a);
    connect(delta, FL.steer);
    connect(delta, FR.steer);
    connect(noDrive.y, tFL.F_drive);
    connect(noDrive.y, tFR.F_drive);
    connect(FL.flange_arm, arbFront.flange_left);
    connect(FR.flange_arm, arbFront.flange_right);
    connect(RL.flange_arm, arbRear.flange_left);
    connect(RR.flange_arm, arbRear.flange_right);
    annotation (Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Polygon(points={{-80,-20},{-80,10},{-50,15},{-30,45},{35,45},{60,15},{85,10},{85,-20},{-80,-20}}, lineColor={0,0,0},
          fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-65,-5},{-25,-45}}, lineColor={0,0,0}, fillColor={50,50,50}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{35,-5},{75,-45}}, lineColor={0,0,0}, fillColor={50,50,50}, fillPattern=FillPattern.Solid),
        Line(points={{-90,-46},{90,-46}}, color={95,95,95}, thickness=1),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>A free sprung body (12 states: position, Cardan angles yaw-pitch-roll and their rates) on four
double-wishbone corners. The front uprights steer about their kingpin axes by the input
<code>delta</code>. The rear tyres get a drive force from a proportional speed controller, so the car
holds <code>v_set</code> through a manoeuvre.</p>
<p>Coil-over preloads are set from the static axle loads and the design motion ratio, so the car
starts close to equilibrium at its design ride height.</p>
<p>Assumed: 1300 kg sprung mass, 2.6 m wheelbase, 1.5 m track, CG 1.2 m behind the front axle and
0.55 m above ground, 55/50 kN/m coil springs (about 23/21 kN/m at the wheel), anti-roll bars of
4000/1500 N.m/rad on the lower-arm angle, 250 kN/m tyres.</p>
</html>"));
  end Vehicle;
end Components;
