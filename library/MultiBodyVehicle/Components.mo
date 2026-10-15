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

  function roadSlope "Longitudinal slope dz/dx of roadHeight"
    extends Modelica.Icons.Function;
    input SI.Position x;
    input SI.Position y;
    input SI.Height h;
    input SI.Length L;
    input SI.Position x0;
    input SI.Position yMin;
    input SI.Position yMax;
    output Real dzdx;
  algorithm
    dzdx := if x < x0 or x > x0 + L or y < yMin or y > yMax then 0
            else h*Modelica.Constants.pi/L*sin(2*Modelica.Constants.pi*(x - x0)/L);
  end roadSlope;

  partial model PartialWheelCorner "Chassis frame, wheel-centre frame, wheel drawing and corner kinematics outputs"
    outer MB.World world;
    parameter Real side=1 "+1 left-hand corner, -1 right-hand (mirrored in y)";
    parameter Boolean animateWheel=true "Draw the wheel; set false when a Tyre draws a spinning one"
      annotation (Dialog(tab="Animation"));
    parameter MB.Types.Color armColor={160,165,175} "Control arms and links" annotation (Dialog(tab="Animation"));
    parameter MB.Types.Color uprightColor={95,100,110} "Upright and spindle" annotation (Dialog(tab="Animation"));
    parameter MB.Types.Color chassisColor={60,60,66} "Chassis brackets and towers" annotation (Dialog(tab="Animation"));
    Visuals.Wheel wheelShape(r=frame_wheel.r_0, R=frame_wheel.R, side=side) if world.enableAnimation and animateWheel
      "Tyre, rim and brake (animation only)"
      annotation (Placement(transformation(extent={{60,-100},{80,-80}})));
    MB.Interfaces.Frame_a frame_a "Chassis reference (axle centre line, wheel-centre height)"
      annotation (Placement(transformation(extent={{-116,-16},{-84,16}})));
    MB.Interfaces.Frame_b frame_wheel "Wheel centre (steers and moves with the upright, does not spin)"
      annotation (Placement(transformation(extent={{84,-16},{116,16}})));
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
  end PartialWheelCorner;

  partial model PartialCorner "Wheel corner with a lower-arm pivot flange"
    extends PartialWheelCorner;
    Modelica.Mechanics.Rotational.Interfaces.Flange_a flange_arm
      "Lower-arm pivot; drive it for a kinematic sweep, leave it open in a vehicle"
      annotation (Placement(transformation(extent={{-10,-110},{10,-90}})));
  end PartialCorner;

  partial model PartialDoubleWishbone "Double-wishbone linkage: lower and upper arms, upright, steering axis"
    extends PartialCorner(r_wheel_design=r_WC);
    parameter SI.Position r_LI[3]={0, 0.25, -0.12} "Lower arm inner pivot" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_LB[3]={0, 0.70, -0.12} "Lower ball joint" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_UI[3]={0, 0.40, 0.16} "Upper arm inner pivot" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_UB[3]={0, 0.66, 0.19} "Upper ball joint" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_WC[3]={0, 0.75, 0} "Wheel centre" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Mass m_lowerArm=6;
    parameter SI.Mass m_upperArm=3;
    parameter SI.Mass m_upright=8 "Upright, hub and brake";
    parameter SI.Mass m_wheel=22 "Wheel and tyre";
    parameter SI.Inertia I_wheel[3]={0.8, 1.4, 0.8} "Wheel inertia about the wheel centre (x, spin, z)";
    parameter Boolean steerable=true "false: steering input is ignored and the upright is locked";
    Modelica.Blocks.Interfaces.RealInput steer(unit="rad") if steerable "Road-wheel steer angle (positive left)"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,110})));

    MB.Parts.FixedTranslation toLI(r=mirror(r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-80,-50},{-60,-30}})));
    MB.Parts.FixedTranslation toUI(r=mirror(r_UI, side), animation=false)
      annotation (Placement(transformation(extent={{-80,30},{-60,50}})));
    MB.Joints.Revolute lowerPivot(n={1,0,0}, useAxisFlange=true, stateSelect=StateSelect.always,
      phi(start=0), w(start=0)) "Lower arm inner pivot (chassis)"
      annotation (Placement(transformation(extent={{-50,-50},{-30,-30}})));
    MB.Parts.BodyShape lowerArm(r=mirror(r_LB - r_LI, side), r_CM=mirror(r_LB - r_LI, side)/2, m=m_lowerArm,
      animation=false)
      annotation (Placement(transformation(extent={{-20,-50},{0,-30}})));
    MB.Joints.Assemblies.JointRRR links(n_a={1,0,0}, rRod1_ia=mirror(r_UB - r_LB, side), rRod2_ib=mirror(r_UB - r_UI, side),
      animation=false)
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
    MB.Parts.FixedTranslation spindle(r=mirror(r_WC - r_LB, side), width=0.05, color=uprightColor) "Upright to wheel centre"
      annotation (Placement(transformation(extent={{76,-50},{96,-30}})));
    MB.Parts.Body wheel(m=m_wheel, r_CM={0,0,0}, I_11=I_wheel[1], I_22=I_wheel[2], I_33=I_wheel[3], animation=false)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={86,-70})));
    Visuals.AArm lowerArmShape(r_ball=mirror(r_LB - r_LI, side), spread=0.17, diameter=0.03, color=armColor)
      "Lower wishbone (animation only)"
      annotation (Placement(transformation(extent={{-20,-30},{0,-10}})));
    Visuals.AArm upperArmShape(r_ball=mirror(r_UB - r_UI, side), spread=0.13, diameter=0.026, color=armColor)
      "Upper wishbone (animation only)"
      annotation (Placement(transformation(extent={{-20,70},{0,90}})));
    Visuals.Bar uprightShape(r_a=-0.03*Modelica.Math.Vectors.normalize(mirror(r_UB - r_LB, side)),
      r_b=mirror(r_UB - r_LB, side) + 0.03*Modelica.Math.Vectors.normalize(mirror(r_UB - r_LB, side)),
      width=0.07, height=0.045, color=uprightColor) "Upright (animation only)"
      annotation (Placement(transformation(extent={{76,-30},{96,-10}})));
  equation
    connect(frame_a, toLI.frame_a);
    connect(frame_a, toUI.frame_a);
    connect(toLI.frame_b, lowerPivot.frame_a);
    connect(lowerPivot.frame_b, lowerArm.frame_a);
    connect(lowerPivot.frame_b, lowerArmShape.frame_a);
    connect(lowerArm.frame_b, links.frame_a);
    connect(links.frame_b, toUI.frame_b);
    connect(links.frame_ia, upright.frame_a);
    connect(links.frame_ib, upperArm.frame_a);
    connect(links.frame_ib, upperArmShape.frame_a);
    connect(links.frame_ia, kingpin.frame_a);
    connect(kingpin.frame_b, spindle.frame_a);
    connect(kingpin.frame_b, uprightShape.frame_a);
    connect(spindle.frame_b, wheel.frame_a);
    connect(spindle.frame_b, frame_wheel);
    connect(lowerPivot.axis, flange_arm);
    connect(steer, steering.phi_ref);
    connect(steering.flange, kingpin.axis);
    connect(steeringLock.flange, kingpin.axis);
    annotation (Documentation(info="<html>
<p>Short-long-arm (SLA) double wishbone in the front view. Lower arm, upright and upper arm
form a planar four-bar with the body. The lower arm pivot is the joint with states; upright and
upper arm are the two rods of a <code>JointRRR</code>, which solves the loop analytically, so the
corner adds no constraint equations and no non-linear loop. The upper arm is
shorter than the lower arm, which gives negative camber gain in bump. The steering joint turns
the spindle about the kingpin axis through the two ball joints (no tie rod: steering is ideal).</p>
<p>Extended by <code>DoubleWishbone</code> (coil-over on the lower arm) and
<code>DoubleWishbonePushrod</code> (pushrod, rocker and inboard coil-over).</p>
</html>"));
  end PartialDoubleWishbone;

  model DoubleWishbone "Double-wishbone corner with an outboard coil-over on the lower arm and a steerable upright"
    extends PartialDoubleWishbone;
    parameter SI.Position r_SL[3]={0, 0.55, -0.12} "Spring seat on the lower arm" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SC[3]={0, 0.50, 0.30} "Spring top mount on the body" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.TranslationalSpringConstant c_spring=55e3 "Coil spring rate" annotation (Dialog(group="Spring and damper"));
    parameter SI.Force F_spring_design=4800 "Spring force at the design position" annotation (Dialog(group="Spring and damper"));
    parameter SI.TranslationalDampingConstant d_damper=4000 "Damper rate along the coil-over" annotation (Dialog(group="Spring and damper"));
    final parameter SI.Length L_spring_design=Modelica.Math.Vectors.length(r_SC - r_SL);
    SI.Length L_spring = coilOver.length "Coil-over length";
    SI.Force F_spring = coilOver.f "Coil-over force (tension positive)";
    MB.Parts.FixedTranslation toSC(r=mirror(r_SC, side), animation=false) "Spring top mount on the chassis"
      annotation (Placement(transformation(extent={{-80,70},{-60,90}})));
    MB.Parts.FixedTranslation toSL(r=mirror(r_SL - r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-20,-80},{0,-60}})));
    MB.Forces.SpringDamperParallel coilOver(c=c_spring, d=d_damper,
      s_unstretched=L_spring_design + F_spring_design/c_spring,
      width=0.08, coilWidth=0.009, numberOfWindings=7, diameter_a=0.045, length_a=0.18, color={230,180,0})
      "Coil spring and damper between lower arm and body"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={20,-80})));
  equation
    connect(frame_a, toSC.frame_a);
    connect(lowerPivot.frame_b, toSL.frame_a);
    connect(toSL.frame_b, coilOver.frame_a);
    connect(coilOver.frame_b, toSC.frame_b);
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
<p>The linkage of <code>PartialDoubleWishbone</code> with an external coil-over: the spring and
damper act between a seat on the lower arm and a mount on the body. Its free length is
set so that it carries <code>F_spring_design</code> at the design position.</p>
</html>"));
  end DoubleWishbone;

  model DoubleWishbonePushrod "Double-wishbone corner with pushrod, bell-crank rocker and inboard coil-over"
    extends PartialDoubleWishbone;
    parameter SI.Position r_PL[3]={0, 0.64, -0.10} "Pushrod pickup on the lower arm" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_RC[3]={0, 0.28, 0.26} "Rocker pivot on the chassis" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_RP[3]={0, 0.355, 0.30} "Pushrod end on the rocker" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_RS[3]={0, 0.325, 0.355} "Spring end on the rocker" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SC[3]={0, 0.07, 0.365} "Inboard spring mount on the chassis" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.TranslationalSpringConstant c_spring=35e3 "Coil spring rate" annotation (Dialog(group="Spring and damper"));
    parameter SI.Force F_spring_design=4300 "Spring force at the design position" annotation (Dialog(group="Spring and damper"));
    parameter SI.TranslationalDampingConstant d_damper=2500 "Damper rate along the coil-over" annotation (Dialog(group="Spring and damper"));
    parameter SI.Mass m_rocker=1.2 "Rocker";
    parameter MB.Types.Color rockerColor={200,40,40} annotation (Dialog(tab="Animation"));
    final parameter SI.Length L_spring_design=Modelica.Math.Vectors.length(r_SC - r_RS);
    SI.Length L_spring = coilOver.length "Coil-over length";
    SI.Force F_spring = coilOver.f "Coil-over force (tension positive)";

    MB.Parts.FixedTranslation toPL(r=mirror(r_PL - r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-20,-80},{0,-60}})));
    MB.Parts.FixedTranslation toRC(r=mirror(r_RC, side), animation=false) "Rocker pivot on the chassis"
      annotation (Placement(transformation(extent={{-80,70},{-60,90}})));
    MB.Parts.FixedTranslation toSC(r=mirror(r_SC, side), animation=false) "Inboard spring mount"
      annotation (Placement(transformation(extent={{-80,90},{-60,110}})));
    MB.Joints.Assemblies.JointRRR rockerLinks(n_a={1,0,0}, rRod1_ia=mirror(r_RP - r_PL, side), rRod2_ib=mirror(r_RP - r_RC, side),
      animation=false) "Pushrod (rod 1) and rocker (rod 2)"
      annotation (Placement(transformation(extent={{-20,-20},{20,20}}, rotation=90, origin={60,60})));
    MB.Parts.Body rocker(m=m_rocker, r_CM=mirror((r_RP + r_RS)/2 - r_RC, side), animation=false)
      annotation (Placement(transformation(extent={{80,60},{100,80}})));
    MB.Parts.FixedTranslation toRS(r=mirror(r_RS - r_RC, side), animation=false)
      annotation (Placement(transformation(extent={{80,80},{100,100}})));
    MB.Forces.SpringDamperParallel coilOver(c=c_spring, d=d_damper,
      s_unstretched=L_spring_design + F_spring_design/c_spring,
      width=0.065, coilWidth=0.008, numberOfWindings=8, diameter_a=0.04, length_a=0.12, color={230,180,0})
      "Inboard coil-over between the rocker and the chassis"
      annotation (Placement(transformation(extent={{40,80},{60,100}})));
    Visuals.Bar pushrodShape(shapeType="cylinder", r_b=mirror(r_RP - r_PL, side), width=0.02, color=armColor)
      "Pushrod (animation only)"
      annotation (Placement(transformation(extent={{20,30},{40,50}})));
    Visuals.Bar rockerShape1(r_b=mirror(r_RP - r_RC, side), width=0.02, height=0.035, color=rockerColor)
      annotation (Placement(transformation(extent={{100,30},{120,50}})));
    Visuals.Bar rockerShape2(r_b=mirror(r_RS - r_RC, side), width=0.02, height=0.035, color=rockerColor)
      annotation (Placement(transformation(extent={{100,10},{120,30}})));
    Visuals.Bar rockerShape3(r_a=mirror(r_RP - r_RC, side), r_b=mirror(r_RS - r_RC, side), width=0.02, height=0.03, color=rockerColor)
      annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    Visuals.Bar rockerPivotShape(shapeType="cylinder", r_a={-0.03, 0, 0}, r_b={0.03, 0, 0}, width=0.04, widthDirection={0,1,0},
      color=chassisColor) "Rocker pivot (animation only)"
      annotation (Placement(transformation(extent={{-60,50},{-40,70}})));
  equation
    connect(lowerPivot.frame_b, toPL.frame_a);
    connect(toPL.frame_b, rockerLinks.frame_a);
    connect(frame_a, toRC.frame_a);
    connect(frame_a, toSC.frame_a);
    connect(toRC.frame_b, rockerLinks.frame_b);
    connect(toRC.frame_b, rockerPivotShape.frame_a);
    connect(rockerLinks.frame_ia, pushrodShape.frame_a);
    connect(rockerLinks.frame_ib, rocker.frame_a);
    connect(rockerLinks.frame_ib, toRS.frame_a);
    connect(rockerLinks.frame_ib, rockerShape1.frame_a);
    connect(rockerLinks.frame_ib, rockerShape2.frame_a);
    connect(rockerLinks.frame_ib, rockerShape3.frame_a);
    connect(toRS.frame_b, coilOver.frame_a);
    connect(coilOver.frame_b, toSC.frame_b);
    annotation (Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Line(points={{-80,-40},{20,-55}}, color={0,0,0}, thickness=1),
        Line(points={{-70,40},{10,35}}, color={0,0,0}, thickness=1),
        Line(points={{20,-55},{10,35}}, color={0,0,0}, thickness=1),
        Line(points={{0,-50},{-50,60}}, color={0,0,0}, thickness=0.5),
        Polygon(points={{-50,60},{-62,80},{-40,74},{-50,60}}, lineColor={0,0,0}, fillColor={200,40,40}, fillPattern=FillPattern.Solid),
        Line(points={{-62,80},{-90,82},{-86,72},{-80,86},{-76,74},{-72,86},{-68,76}}, color={0,0,255}),
        Rectangle(extent={{30,70},{70,-70}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid, radius=8),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>Race-car layout with an inboard spring. A pushrod runs from the lower arm up to a bell-crank
rocker on the chassis, and the rocker compresses a coil-over lying across the top of the chassis.
The pushrod and the rocker form a second planar four-bar with the lower arm and the chassis,
closed analytically by another <code>JointRRR</code>, so the corner still has one state.</p>
<p>The rocker sets the motion ratio (spring travel per wheel travel) and lets it vary with travel
(a rising or falling rate); compare it with the outboard coil-over in
<code>Examples.PushrodVsCoilOver</code>.</p>
</html>"));
  end DoubleWishbonePushrod;

  model MacPherson "MacPherson strut corner: telescopic strut with coil-over, lower control arm"
    extends PartialCorner(r_wheel_design=r_WC);
    parameter SI.Position r_TM[3]={0, 0.58, 0.60} "Strut top mount" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SB[3]={0, 0.66, 0.08} "Strut-to-upright clamp" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_LI[3]={0, 0.28, -0.12} "Lower arm inner pivot" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_LB[3]={0, 0.71, -0.14} "Lower ball joint" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_WC[3]={0, 0.76, 0} "Wheel centre" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.TranslationalSpringConstant c_spring=35e3 "Coil spring rate" annotation (Dialog(group="Spring and damper"));
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

    MB.Parts.FixedTranslation toTM(r=mirror(r_TM, side), width=0.04, color=chassisColor) "Strut tower on the chassis"
      annotation (Placement(transformation(extent={{-80,50},{-60,70}})));
    MB.Parts.FixedTranslation toLI(r=mirror(r_LI, side), animation=false)
      annotation (Placement(transformation(extent={{-80,-50},{-60,-30}})));
    MB.Joints.Revolute topMount(n={1,0,0}) "Strut top mount (front-view hinge)"
      annotation (Placement(transformation(extent={{-50,50},{-30,70}})));
    MB.Parts.Body strutBody(m=m_strut, r_CM={0,0,0}, animation=false)
      annotation (Placement(transformation(extent={{-30,80},{-10,100}})));
    MB.Joints.Prismatic strut(n=Modelica.Math.Vectors.normalize(mirror(r_SB - r_TM, side)), useAxisFlange=true,
      s(start=L_strut_design), boxWidth=0.06, boxColor={230,180,0}) "Telescopic damper"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,40})));
    Modelica.Mechanics.Translational.Components.SpringDamper coilOver(c=c_spring, d=d_damper,
      s_rel0=L_strut_design + F_spring_design/c_spring) "Coil spring and damper along the strut"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={30,40})));
    MB.Parts.BodyShape upright(r=mirror(r_LB - r_SB, side), r_CM=mirror(r_WC - r_SB, side)*0.7, m=m_upright,
      width=0.05, color=uprightColor, animateSphere=false)
      "From the strut clamp down to the lower ball joint"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,0})));
    MB.Joints.RevolutePlanarLoopConstraint lowerBall(n={1,0,0}) "Lower ball joint; closes the planar loop"
      annotation (Placement(transformation(extent={{10,-50},{-10,-30}})));
    MB.Parts.BodyShape lowerArm(r=mirror(r_LB - r_LI, side), r_CM=mirror(r_LB - r_LI, side)/2, m=m_lowerArm,
      width=0.035, color=armColor, animateSphere=false)
      annotation (Placement(transformation(extent={{-30,-50},{-10,-30}})));
    MB.Joints.Revolute lowerPivot(n={1,0,0}, useAxisFlange=true, stateSelect=StateSelect.always,
      phi(start=0), w(start=0)) "Lower arm inner pivot (chassis)"
      annotation (Placement(transformation(extent={{-56,-50},{-36,-30}})));
    MB.Parts.FixedTranslation spindle(r=mirror(r_WC - r_SB, side), width=0.05, color=uprightColor) "Upright to wheel centre"
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

  model CompliantLink "Rod link with ball joints at both ends and a stiff rubber bush along its axis"
    extends MB.Interfaces.PartialLineForce;
    parameter SI.Length L0 "Unloaded length (centre to centre)";
    parameter SI.TranslationalSpringConstant c=2e6 "Axial stiffness of the link and its bushes";
    parameter SI.TranslationalDampingConstant d=2000 "Axial damping";
    parameter Boolean animation=true;
    parameter SI.Diameter diameter=0.022 "Rod diameter";
    parameter MB.Types.Color color={160,165,175};
    SI.Length deflection = s - L0 "Elastic stretch (positive in tension)";
  protected
    MB.Visualizers.Advanced.Shape rod(shapeType="cylinder", r=frame_a.r_0, R=MB.Frames.nullRotation(),
      lengthDirection=frame_b.r_0 - frame_a.r_0, widthDirection={0,0,1}, length=s, width=diameter, height=diameter,
      color=color, specularCoefficient=0.8) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape ball_a(shapeType="sphere", r=frame_a.r_0, R=MB.Frames.nullRotation(), r_shape={-0.018, 0, 0},
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=0.036, width=0.036, height=0.036,
      color={45,45,50}, specularCoefficient=0.5) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape ball_b(shapeType="sphere", r=frame_b.r_0, R=MB.Frames.nullRotation(), r_shape={-0.018, 0, 0},
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=0.036, width=0.036, height=0.036,
      color={45,45,50}, specularCoefficient=0.5) if world.enableAnimation and animation;
  equation
    f = c*(s - L0) + d*der(s);
    annotation (Icon(graphics={
        Line(points={{-80,0},{80,0}}, color={0,0,0}, thickness=1),
        Ellipse(extent={{-96,16},{-64,-16}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{64,16},{96,-16}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,60},{150,20}}, textString="%name", textColor={0,0,255})}));
  end CompliantLink;

  model Multilink "Five-link rear corner, elasto-kinematic: a free upright held by five stiff links and a coil-over"
    extends PartialWheelCorner(r_wheel_design=r_WC);
    parameter SI.Position r_WC[3]={0, 0.78, 0} "Wheel centre" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_C[5,3]={{0.12, 0.345, 0.159}, {-0.22, 0.377, 0.147}, {0.28, 0.33, -0.118}, {-0.22, 0.36, -0.069}, {-0.30, 0.385, -0.027}}
      "Chassis ends: upper front, upper rear, lower front, lower rear, toe link" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_U[5,3]={{0.04, 0.69, 0.16}, {-0.06, 0.69, 0.16}, {0.05, 0.74, -0.13}, {-0.07, 0.75, -0.12}, {-0.16, 0.72, -0.062}}
      "Upright ends, same order" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SC[3]={0, 0.50, 0.40} "Spring top mount on the body" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.Position r_SU[3]={0, 0.60, -0.08} "Spring seat on the upright" annotation (Dialog(group="Hardpoints (left-hand corner)"));
    parameter SI.TranslationalSpringConstant c_link=1e7 "Axial stiffness of each link with its bushes" annotation (Dialog(group="Links"));
    parameter SI.TranslationalDampingConstant d_link=5000 "Axial damping of each link" annotation (Dialog(group="Links"));
    parameter SI.TranslationalSpringConstant c_spring=40e3 "Coil spring rate" annotation (Dialog(group="Spring and damper"));
    parameter SI.Force F_spring_design=3500 "Spring force at the design position" annotation (Dialog(group="Spring and damper"));
    parameter SI.TranslationalDampingConstant d_damper=3000 "Damper rate along the coil-over" annotation (Dialog(group="Spring and damper"));
    parameter SI.Mass m_upright=8 "Upright, hub and brake";
    parameter SI.Mass m_wheel=22 "Wheel and tyre";
    parameter SI.Inertia I_wheel[3]={0.8, 1.4, 0.8} "Wheel inertia about the wheel centre (x, spin, z)";
    final parameter SI.Length L_spring_design=Modelica.Math.Vectors.length(r_SC - r_SU);
    SI.Length L_spring = coilOver.length "Coil-over length";
    SI.Force F_spring = coilOver.f "Coil-over force (tension positive)";
    SI.Force F_link[5] = {link[i].f for i in 1:5} "Link forces (tension positive)";

    MB.Parts.FixedTranslation toWC(r=mirror(r_WC, side), animation=false)
      annotation (Placement(transformation(extent={{-80,-10},{-60,10}})));
    MB.Joints.FreeMotion free(useQuaternions=false, sequence_angleStates={1,2,3},
      r_rel_a(start={0, 0, 0}, each fixed=true), v_rel_a(start={0, 0, 0}, each fixed=true),
      angles_start={0, 0, 0}, angles_fixed=true, w_rel_a_fixed=true, animation=false)
      "Six free coordinates of the upright relative to its design position"
      annotation (Placement(transformation(extent={{-40,-10},{-20,10}})));
    MB.Parts.Body upright(m=m_upright + m_wheel, r_CM={0,0,0}, I_11=I_wheel[1] + 0.1, I_22=I_wheel[2] + 0.1, I_33=I_wheel[3] + 0.1,
      animation=false) "Upright with the wheel, frame_a at the wheel centre"
      annotation (Placement(transformation(extent={{0,-40},{20,-20}})));
    MB.Parts.FixedTranslation toC[5](r={mirror(r_C[i, :], side) for i in 1:5}, each animation=false)
      annotation (Placement(transformation(extent={{-80,30},{-60,50}})));
    MB.Parts.FixedTranslation toU[5](r={mirror(r_U[i, :] - r_WC, side) for i in 1:5}, each animation=false)
      annotation (Placement(transformation(extent={{20,30},{40,50}})));
    CompliantLink link[5](L0={Modelica.Math.Vectors.length(r_U[i, :] - r_C[i, :]) for i in 1:5}, each c=c_link, each d=d_link,
      each color=armColor)
      annotation (Placement(transformation(extent={{-30,30},{-10,50}})));
    MB.Parts.FixedTranslation toSC(r=mirror(r_SC, side), animation=false)
      annotation (Placement(transformation(extent={{-80,60},{-60,80}})));
    MB.Parts.FixedTranslation toSU(r=mirror(r_SU - r_WC, side), animation=false)
      annotation (Placement(transformation(extent={{20,60},{40,80}})));
    MB.Forces.SpringDamperParallel coilOver(c=c_spring, d=d_damper,
      s_unstretched=L_spring_design + F_spring_design/c_spring,
      width=0.075, coilWidth=0.009, numberOfWindings=7, diameter_a=0.045, length_a=0.16, color={230,180,0})
      "Coil spring and damper between the upright and the body"
      annotation (Placement(transformation(extent={{-30,60},{-10,80}})));
    Visuals.Bar knuckle[5](r_b={mirror(r_U[i, :] - r_WC, side) for i in 1:5}, each width=0.035, each height=0.035,
      each widthDirection={1,0,0.3}, each color=uprightColor) "Upright arms out to the link ends (animation only)"
      annotation (Placement(transformation(extent={{40,0},{60,20}})));
    Visuals.Bar seat(r_b=mirror(r_SU - r_WC, side), width=0.04, height=0.04, widthDirection={1,0,0.3}, color=uprightColor)
      annotation (Placement(transformation(extent={{40,-30},{60,-10}})));
    Visuals.Bar hub(shapeType="cylinder", r_a=mirror({0, -0.1, 0}, side), r_b=mirror({0, 0.02, 0}, side), width=0.09,
      widthDirection={1,0,0}, color=uprightColor)
      annotation (Placement(transformation(extent={{40,-60},{60,-40}})));
  equation
    connect(frame_a, toWC.frame_a);
    connect(toWC.frame_b, free.frame_a);
    connect(free.frame_b, upright.frame_a);
    connect(free.frame_b, frame_wheel);
    connect(free.frame_b, seat.frame_a);
    connect(free.frame_b, hub.frame_a);
    for i in 1:5 loop
      connect(frame_a, toC[i].frame_a);
      connect(toC[i].frame_b, link[i].frame_a);
      connect(link[i].frame_b, toU[i].frame_b);
      connect(toU[i].frame_a, free.frame_b);
      connect(free.frame_b, knuckle[i].frame_a);
    end for;
    connect(frame_a, toSC.frame_a);
    connect(toSC.frame_b, coilOver.frame_b);
    connect(coilOver.frame_a, toSU.frame_b);
    connect(toSU.frame_a, free.frame_b);
    annotation (Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Line(points={{-80,50},{10,40}}, color={0,0,0}, thickness=1),
        Line(points={{-80,30},{10,30}}, color={0,0,0}, thickness=1),
        Line(points={{-80,-30},{10,-30}}, color={0,0,0}, thickness=1),
        Line(points={{-80,-50},{10,-40}}, color={0,0,0}, thickness=1),
        Line(points={{-80,0},{10,0}}, color={0,0,0}, thickness=0.5),
        Rectangle(extent={{30,70},{70,-70}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid, radius=8),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>A five-link independent rear corner in 3-D: two upper links, two lower links and a toe link,
each a rod with ball joints at both ends. The links are not rigid: each is a
<code>CompliantLink</code>, a stiff axial spring standing for the rod and its rubber bushes
(10 kN/mm by default). The upright is a free body with six coordinates, so the corner is
elasto-kinematic like the Modelon VDL and VeSyMA elasto-kinematic templates: camber and toe
come out of the link geometry and change a little under load as the bushes deflect.</p>
<p>A rigid five-link version closes five spherical loops at once; modelling the links as
stiff springs avoids the constraint equations and keeps the model fast and robust.</p>
</html>"));
  end Multilink;

  model Tyre "Point-contact tyre: vertical spring-damper with lift-off, Pacejka-type lateral force, rolling resistance"
    import Modelica.Math.Vectors.normalize;
    outer MB.World world;
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
    parameter Boolean usePad=false "Stand on a wheel pad: road height and rate come from the inputs z_pad, v_pad"
      annotation (Dialog(group="Road"), Evaluate=true);
    parameter Boolean animation=false "Draw a wheel that spins as the tyre rolls" annotation (Dialog(tab="Animation"));
    parameter Real side=1 "+1 left, -1 right: which way the rim faces" annotation (Dialog(tab="Animation"));
    Modelica.Blocks.Interfaces.RealInput z_pad(unit="m") if usePad "Wheel-pad height"
      annotation (Placement(transformation(extent={{-140,-80},{-100,-40}})));
    Modelica.Blocks.Interfaces.RealInput v_pad(unit="m/s") if usePad "Wheel-pad vertical speed"
      annotation (Placement(transformation(extent={{-140,-120},{-100,-80}})));

    SI.Position r[3] = frame_a.r_0 "Wheel centre";
    SI.Velocity v[3] = der(frame_a.r_0) "Wheel centre velocity";
    SI.Height z_road = if usePad then z_pad_internal else roadHeight(r[1] + v_belt*time, r[2], bump_h, bump_L, bump_x0, bump_yMin, bump_yMax);
    SI.Length deflection = R0 - (r[3] - z_road) "Radial tyre deflection";
    SI.Velocity deflection_rate = (if usePad then v_pad_internal
      else roadSlope(r[1] + v_belt*time, r[2], bump_h, bump_L, bump_x0, bump_yMin, bump_yMax)*(v[1] + v_belt)) - v[3]
      "Rate of deflection, including the road surface rising under the wheel";
    SI.Velocity v_long "Contact speed along the wheel heading, relative to the road";
    SI.Velocity v_lat "Contact speed across the wheel, relative to the road";
    SI.Angle alpha "Slip angle";
    SI.Force Fz "Vertical load";
    SI.Force Fy "Lateral force (wheel axes)";
    SI.Force Fx "Longitudinal force (wheel axes)";
    SI.Force F[3] "Force on the wheel, world axes";
    SI.Angle phi_spin(start=0, fixed=true) "Wheel rotation angle from rolling without slip (for the animation)";
  protected
    Modelica.Blocks.Interfaces.RealInput z_pad_internal(unit="m");
    Modelica.Blocks.Interfaces.RealInput v_pad_internal(unit="m/s");
    Visuals.Wheel wheelShape(r=frame_a.r_0, R=frame_a.R, phi=phi_spin, side=side) if world.enableAnimation and animation;
    Real e_x[3] = MB.Frames.resolve1(frame_a.R, {1, 0, 0});
    Real e_long[3] = normalize({e_x[1], e_x[2], 0});
    Real e_lat[3] = {-e_long[2], e_long[1], 0};
    SI.Position r_contact[3] = {0, 0, -(r[3] - z_road)} "Contact point relative to the wheel centre";
  equation
    connect(z_pad, z_pad_internal);
    connect(v_pad, v_pad_internal);
    if not usePad then
      z_pad_internal = 0;
      v_pad_internal = 0;
    end if;
    der(phi_spin) = v_long/R0;
    v_long = (v + {v_belt, 0, 0})*e_long;
    v_lat = v*e_lat;
    alpha = atan(v_lat/(abs(v_long) + v_eps));
    Fz = if deflection > 0 then max(0, c_z*deflection + d_z*deflection_rate) else 0;
    Fy = -mu*Fz*sin(C*atan(B*alpha));
    Fx = if deflection > 0 then F_drive - f_rr*Fz*tanh(v_long/v_eps) else 0 "No drive force while airborne";
    F = Fx*e_long + Fy*e_lat + {0, 0, Fz};
    frame_a.f = -MB.Frames.resolve2(frame_a.R, F);
    frame_a.t = -MB.Frames.resolve2(frame_a.R, cross(r_contact, F));
    annotation (Icon(graphics={
        Ellipse(extent={{-60,90},{60,-90}}, lineColor={0,0,0}, fillColor={50,50,50}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-30,45},{30,-45}}, lineColor={0,0,0}, fillColor={200,200,200}, fillPattern=FillPattern.Solid),
        Line(points={{-100,-95},{100,-95}}, color={95,95,95}, thickness=1),
        Text(extent={{-150,-100},{150,-140}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>The wheel has no spin degree of freedom: the frame follows the upright. For the animation the
tyre integrates a rolling angle <code>phi_spin</code> from the contact speed and, with
<code>animation=true</code>, draws a treaded tyre, rim and brake that turn by it. The contact point is taken straight
below the wheel centre on the road surface, and the force is applied there, so it also loads the
suspension with the correct moment arm.</p>
<ul>
<li>Vertical: <code>Fz = max(0, c_z*deflection + d_z*d(deflection)/dt)</code>, zero when the tyre leaves the road.
    The deflection rate includes the road rising under the moving wheel.</li>
<li>Lateral: <code>Fy = -mu*Fz*sin(C*atan(B*alpha))</code>, with slip angle
    <code>alpha = atan(v_lat/(|v_long| + v_eps))</code>.</li>
<li>Longitudinal: the drive/brake input minus rolling resistance <code>f_rr*Fz</code>, and nothing
    while the tyre is off the road.</li>
</ul>
<p>The road is flat with an optional half-cosine bump between <code>bump_yMin</code> and
<code>bump_yMax</code>. With <code>v_belt</code> &gt; 0 the road moves under a stationary wheel,
as on a flat-belt test rig. With <code>usePad=true</code> the road is a wheel pad of a post rig
whose height and speed come from the inputs <code>z_pad</code> and <code>v_pad</code>.</p>
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

  model WheelPad "Hydraulic wheel pad of a post rig: follows a height demand through a second-order servo loop"
    outer MB.World world;
    parameter SI.Position r_pad[2] "Pad centre, x and y in the world frame";
    parameter SI.Height h0=0.5 "Height of the pad surface at rest above the floor";
    parameter SI.Frequency f_servo=25 "Servo bandwidth";
    parameter Real zeta=0.8 "Servo damping ratio";
    parameter SI.Length padSize=0.42 "Pad width and length";
    parameter Boolean animation=true;
    Modelica.Blocks.Interfaces.RealInput z_ref(unit="m") "Pad displacement demand"
      annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    Modelica.Blocks.Interfaces.RealOutput z(unit="m") "Pad surface height"
      annotation (Placement(transformation(extent={{100,30},{120,50}})));
    Modelica.Blocks.Interfaces.RealOutput v(unit="m/s") "Pad vertical speed"
      annotation (Placement(transformation(extent={{100,-50},{120,-30}})));
    SI.Position s(start=0, fixed=true) "Pad displacement";
    SI.Velocity v_s(start=0, fixed=true) "Pad speed";
  protected
    constant Real pi=Modelica.Constants.pi;
    MB.Visualizers.Advanced.Shape base(shapeType="box", r={r_pad[1], r_pad[2], 0}, R=MB.Frames.nullRotation(),
      lengthDirection={0,0,1}, widthDirection={1,0,0}, length=0.06, width=0.6, height=0.6,
      color={70,75,85}, specularCoefficient=0.3) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape cylinder(shapeType="cylinder", r={r_pad[1], r_pad[2], 0.06}, R=MB.Frames.nullRotation(),
      lengthDirection={0,0,1}, widthDirection={1,0,0}, length=h0 - 0.24, width=0.2, height=0.2,
      color={30,90,170}, specularCoefficient=0.6) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape rod(shapeType="cylinder", r={r_pad[1], r_pad[2], h0 - 0.2}, R=MB.Frames.nullRotation(),
      lengthDirection={0,0,1}, widthDirection={1,0,0}, length=0.17 + s, width=0.085, height=0.085,
      color={215,220,228}, specularCoefficient=1) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape pad(shapeType="box", r={r_pad[1], r_pad[2], z - 0.035}, R=MB.Frames.nullRotation(),
      lengthDirection={0,0,1}, widthDirection={1,0,0}, length=0.035, width=padSize, height=padSize,
      color={45,45,50}, specularCoefficient=0.4) if world.enableAnimation and animation;
  equation
    der(s) = v_s;
    der(v_s) = (2*pi*f_servo)^2*(z_ref - s) - 2*zeta*(2*pi*f_servo)*v_s;
    z = h0 + s;
    v = v_s;
    annotation (Icon(graphics={
        Rectangle(extent={{-60,-60},{60,-80}}, lineColor={0,0,0}, fillColor={70,75,85}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-30,-60},{30,20}}, lineColor={0,0,0}, fillColor={30,90,170}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-10,20},{10,60}}, lineColor={0,0,0}, fillColor={215,220,228}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-70,60},{70,72}}, lineColor={0,0,0}, fillColor={45,45,50}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,120},{150,80}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>A servo-hydraulic actuator under one tyre. The pad follows <code>z_ref</code> through a
second-order loop (25 Hz, damping 0.8), which also gives the pad speed that a tyre standing on it
needs (<code>Tyre(usePad=true)</code>). The pad is a moving road: the tyre force does not slow it.</p>
</html>"));
  end WheelPad;

  model AeroLoader "Pull-down actuator of a seven-post rig: a vertical force on the body from the floor"
    outer MB.World world;
    parameter Boolean animation=true;
    MB.Interfaces.Frame_b frame_b "Attachment on the body"
      annotation (Placement(transformation(extent={{84,-16},{116,16}})));
    Modelica.Blocks.Interfaces.RealInput F(unit="N") "Downforce (pull-down positive)"
      annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
  protected
    MB.Visualizers.Advanced.Shape cylinder(shapeType="cylinder", r={frame_b.r_0[1], frame_b.r_0[2], 0}, R=MB.Frames.nullRotation(),
      lengthDirection={0,0,1}, widthDirection={1,0,0}, length=0.22, width=0.12, height=0.12,
      color={200,120,20}, specularCoefficient=0.6) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape rod(shapeType="cylinder", r={frame_b.r_0[1], frame_b.r_0[2], 0.2}, R=MB.Frames.nullRotation(),
      lengthDirection={0,0,1}, widthDirection={1,0,0}, length=frame_b.r_0[3] - 0.2, width=0.035, height=0.035,
      color={215,220,228}, specularCoefficient=1) if world.enableAnimation and animation;
  equation
    frame_b.f = MB.Frames.resolve2(frame_b.R, {0, 0, F});
    frame_b.t = zeros(3);
    annotation (Icon(graphics={
        Rectangle(extent={{-30,-80},{30,0}}, lineColor={0,0,0}, fillColor={200,120,20}, fillPattern=FillPattern.Solid),
        Line(points={{0,0},{0,60},{84,60},{84,0}}, color={0,0,0}),
        Text(extent={{-150,120},{150,80}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>A seven-post rig loads the body with three extra actuators that pull it down, standing for
the aerodynamic downforce. This one applies <code>F</code> straight down at <code>frame_b</code>
and draws an actuator from the floor up to the attachment point.</p>
</html>"));
  end AeroLoader;

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
    parameter Boolean onRig=false "Tyres stand on the wheel pads of a post rig (inputs z_pad, v_pad) instead of the road"
      annotation (Evaluate=true);
    parameter SI.Height z_ground=0 "Height of the road or pads under the tyres at the start";
    parameter MB.Types.Color bodyColor={200,25,35} "Body shell" annotation (Dialog(tab="Animation"));
    final parameter SI.Force W_front=m_body*Modelica.Constants.g_n*(wheelbase - a_CG)/wheelbase/2 "Sprung load per front wheel";
    final parameter SI.Force W_rear=m_body*Modelica.Constants.g_n*a_CG/wheelbase/2 "Sprung load per rear wheel";
    final parameter SI.Mass m_corner=FL.m_wheel + FL.m_upright + FL.m_lowerArm + FL.m_upperArm "Moving mass of one corner";
    final parameter SI.Force F_armShare=(FL.m_lowerArm + FL.m_upperArm)/2*Modelica.Constants.g_n
      "Arm weight carried by the spring";
    final parameter SI.Length dz_front=(W_front + m_corner*Modelica.Constants.g_n)/tFL.c_z "Static front tyre deflection";
    final parameter SI.Length dz_rear=(W_rear + m_corner*Modelica.Constants.g_n)/tRL.c_z "Static rear tyre deflection";
    final parameter SI.Height z_start=z_ground + tFL.R0 - dz_front "Initial height of the front axle line (wheel centres at design)";
    final parameter SI.Angle pitch_start=atan((dz_front - dz_rear)/wheelbase) "Initial pitch so all four tyres start at static load";

    Modelica.Blocks.Interfaces.RealInput delta(unit="rad") "Front road-wheel steer angle (positive left)"
      annotation (Placement(transformation(extent={{-120,-10},{-100,10}})));
    Modelica.Blocks.Interfaces.RealInput z_pad[4](each unit="m") if onRig "Wheel-pad heights FL, FR, RL, RR"
      annotation (Placement(transformation(extent={{-120,-50},{-100,-30}})));
    Modelica.Blocks.Interfaces.RealInput v_pad[4](each unit="m/s") if onRig "Wheel-pad speeds FL, FR, RL, RR"
      annotation (Placement(transformation(extent={{-120,-90},{-100,-70}})));
    MB.Interfaces.Frame_b frame_body "Sprung body (front axle centre line, wheel-centre height), for rig loads"
      annotation (Placement(transformation(extent={{84,-16},{116,16}})));

    MB.Parts.Body body(m=m_body, r_CM={-a_CG, 0, h_CG}, I_11=I_roll, I_22=I_pitch, I_33=I_yaw,
      enforceStates=true, useQuaternions=false, sequence_angleStates={3,2,1},
      r_0(start={0, 0, z_start}, each fixed=true), v_0(start={v_start, 0, 0}, each fixed=true),
      angles_fixed=true, angles_start={0, pitch_start, 0}, w_0_fixed=true, animation=false)
      "Sprung body; frame_a on the front axle centre line at wheel-centre height"
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=90, origin={0,50})));
    Visuals.BodyShell shell(color=bodyColor) "Monocoque, sidepods, engine cover, wings and driver (animation only)"
      annotation (Placement(transformation(extent={{-60,90},{-40,110}})));
    MB.Parts.FixedTranslation toRear(r={-wheelbase, 0, 0}, animation=false)
      annotation (Placement(transformation(extent={{-10,-10},{10,10}}, rotation=270, origin={0,-10})));
    DoubleWishbone FL(animateWheel=false, side=1, c_spring=c_front, d_damper=d_front, F_spring_design=(W_front + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-30,40},{-50,60}})));
    DoubleWishbone FR(animateWheel=false, side=-1, c_spring=c_front, d_damper=d_front, F_spring_design=(W_front + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{30,40},{50,60}})));
    DoubleWishbone RL(animateWheel=false, side=1, steerable=false, c_spring=c_rear, d_damper=d_rear, F_spring_design=(W_rear + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{-30,-50},{-50,-30}})));
    DoubleWishbone RR(animateWheel=false, side=-1, steerable=false, c_spring=c_rear, d_damper=d_rear, F_spring_design=(W_rear + F_armShare)/motionRatio,
      lowerPivot(phi(fixed=true), w(fixed=true)))
      annotation (Placement(transformation(extent={{30,-50},{50,-30}})));
    Tyre tFL(usePad=onRig, animation=true, side=1, bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{-70,20},{-90,40}})));
    Tyre tFR(usePad=onRig, animation=true, side=-1, bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{70,20},{90,40}})));
    Tyre tRL(usePad=onRig, animation=true, side=1, bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
      annotation (Placement(transformation(extent={{-70,-70},{-90,-50}})));
    Tyre tRR(usePad=onRig, animation=true, side=-1, bump_h=bump_h, bump_L=bump_L, bump_x0=bump_x0, bump_yMin=bump_yMin, bump_yMax=bump_yMax)
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
    Modelica.Blocks.Interfaces.RealInput z_pad_internal[4](each unit="m");
    Modelica.Blocks.Interfaces.RealInput v_pad_internal[4](each unit="m/s");
    Real e_fwd[3] = MB.Frames.resolve1(body.frame_a.R, {1, 0, 0});
    Real e_left[3] = MB.Frames.resolve1(body.frame_a.R, {0, 1, 0});
    SI.AngularVelocity w_body[3] = MB.Frames.angularVelocity2(body.frame_a.R);
  equation
    connect(body.frame_a, shell.frame_a);
    connect(body.frame_a, frame_body);
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
    connect(z_pad, z_pad_internal);
    connect(v_pad, v_pad_internal);
    if not onRig then
      z_pad_internal = zeros(4);
      v_pad_internal = zeros(4);
    end if;
    connect(z_pad_internal[1], tFL.z_pad);
    connect(z_pad_internal[2], tFR.z_pad);
    connect(z_pad_internal[3], tRL.z_pad);
    connect(z_pad_internal[4], tRR.z_pad);
    connect(v_pad_internal[1], tFL.v_pad);
    connect(v_pad_internal[2], tFR.v_pad);
    connect(v_pad_internal[3], tRL.v_pad);
    connect(v_pad_internal[4], tRR.v_pad);
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
<p>With <code>onRig=true</code> the tyres stand on the wheel pads of a post rig instead of the
road (see <code>Examples.SevenPostRig</code>); <code>frame_body</code> gives access to the body for
rig loads such as aero pull-down actuators.</p>
</html>"));
  end Vehicle;
end Components;
