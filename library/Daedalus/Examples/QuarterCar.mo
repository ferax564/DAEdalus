within Daedalus.Examples;
package QuarterCar "Quarter-car ride model with a physical hydraulic damper (after Claytex VeSyMA)"
  extends Daedalus.Icons.ExamplesPackage;
  import H = Daedalus.Hydraulics;
  import Tr = Daedalus.Mechanics.Translational;

  block RoadBump "Half-cosine bump of height h and length L crossed at speed v"
    extends Daedalus.Blocks.Interfaces.SO(y(unit="m"));
    parameter Units.Length h=0.05 "Bump height";
    parameter Units.Length L=0.5 "Bump length";
    parameter Units.Velocity v=10 "Vehicle speed";
    parameter Units.Time t0=0.2 "Time the tyre reaches the bump";
  equation
    y = if time < t0 or time > t0 + L/v then 0 else h/2*(1 - cos(2*3.141592653589793*(time - t0)*v/L));
    annotation (Icon(graphics={
        Line(points={{-80,-60},{-30,-60},{-20,-50},{-10,10},{0,30},{10,10},{20,-50},{30,-60},{80,-60}}, color={0,0,127}, smooth=Smooth.Bezier),
        Line(points={{-90,-70},{90,-70}}, color={95,95,95})}));
  end RoadBump;

  model MonotubeDamper "Gas-charged monotube damper: piston valves, bleed orifice and floating gas piston"
    extends Tr.Interfaces.PartialTwoFlanges;
    parameter Real x_air=0 "Entrained air in the oil";
    parameter Units.Length D_piston=0.046;
    parameter Units.Length d_rod=0.018;
    parameter Units.Length stroke=0.20;
    parameter Units.Length L_mid=0.3 "Distance between the flanges at mid-stroke";
    parameter Units.AbsolutePressure p_gas=20e5 "Gas charge";
    parameter Units.Area A_bleed=1.5e-6 "Bypass bleed (sets low-speed damping)";
    parameter Units.Area A_rebound=2.5e-5 "Rebound valve fully open";
    parameter Units.Area A_compression=4e-5 "Compression valve fully open";
    parameter Units.PressureDifference dp_rebound=8e5 "Rebound valve preload";
    parameter Units.PressureDifference dp_compression=4e5 "Compression valve preload";
    H.Actuators.Cylinder body(D_piston=D_piston, d_rod_A=0, d_rod_B=d_rod, stroke=stroke, fixInitialExtension=false, L_closed=L_mid - stroke/2,
      V_dead=5e-6, x_air=x_air, pA_start=p_gas, pB_start=p_gas, d_visc=50, F_coulomb=30)
      "A: chamber below the piston (full bore); B: rod side";
    H.Components.Orifice bleed(A=A_bleed, Cd=0.7);
    H.Components.CheckValve reboundValve(A_max=A_rebound, dp_crack=dp_rebound, dp_open=20e5) "Opens B -> A on extension";
    H.Components.CheckValve compressionValve(A_max=A_compression, dp_crack=dp_compression, dp_open=15e5) "Opens A -> B on compression";
    H.Components.Accumulator gasChamber(V0=1.5e-4, p0=0.98*p_gas, n=1.0, p_start=p_gas) "Floating piston and nitrogen charge";
    Units.Force F_damper "Damper force (compression positive)";
    Units.Velocity v_damper "Extension speed";
  equation
    F_damper = body.f;
    v_damper = body.v;
    connect(flange_a, body.housing);
    connect(body.rod, flange_b);
    connect(body.port_A, bleed.port_a);
    connect(bleed.port_b, body.port_B);
    connect(body.port_B, reboundValve.port_a);
    connect(reboundValve.port_b, body.port_A);
    connect(body.port_A, compressionValve.port_a);
    connect(compressionValve.port_b, body.port_B);
    connect(body.port_A, gasChamber.port);
  end MonotubeDamper;

  model Corner "Sprung mass, suspension spring, damper, unsprung mass and tyre over a road actuator"
    parameter Units.Mass m_body=400 "Quarter of the body mass";
    parameter Units.Mass m_wheel=40 "Wheel, hub and brake";
    parameter Units.TranslationalSpringConstant c_spring=30e3;
    parameter Units.TranslationalSpringConstant c_tyre=250e3;
    parameter Units.TranslationalDampingConstant d_tyre=100;
    parameter Boolean hydraulicDamper=true "false: linear damper with d_linear";
    parameter Units.TranslationalDampingConstant d_linear=2000;
    parameter Real x_air=0;
    Tr.Components.Mass body(m=m_body, g=-9.81);
    Tr.Components.Mass wheel(m=m_wheel, g=-9.81);
    Tr.Components.Spring spring(c=c_spring, s_rel0=0.3 + (m_body*9.81 - F_gas)/c_spring) "Preloaded to carry the body";
    Tr.Components.ElastoGap tyre(c=c_tyre, d=d_tyre, s_rel0=0.3 + (m_body + m_wheel)*9.81/c_tyre) "Tyre can lift off the road";
    MonotubeDamper damper(x_air=x_air, p_gas=p_gas, d_rod=d_rod) if hydraulicDamper;
    Tr.Components.Damper linearDamper(d=d_linear) if not hydraulicDamper;
    Tr.Sources.Position road(f_crit=200);
    parameter Units.AbsolutePressure p_gas=20e5 "Damper gas charge";
    parameter Units.Length d_rod=0.018 "Damper rod diameter";
    parameter Units.Force F_gas=if hydraulicDamper then p_gas*3.141592653589793/4*d_rod^2 else 0
      "Gas spring force of the damper at the static position";
    Daedalus.Blocks.Interfaces.RealInput z_road(unit="m") annotation (Placement(transformation(extent={{-20,-20},{20,20}}, rotation=90, origin={0,-120})));
    Units.Acceleration a_body = body.a "Vertical body acceleration (ride comfort)";
    Units.Force F_tyre = -tyre.f "Tyre load (road holding)";
  equation
    connect(z_road, road.s_ref);
    connect(road.flange, tyre.flange_a);
    connect(tyre.flange_b, wheel.flange_a);
    connect(wheel.flange_b, spring.flange_a);
    connect(spring.flange_b, body.flange_a);
    connect(wheel.flange_b, damper.flange_a);
    connect(damper.flange_b, body.flange_a);
    connect(wheel.flange_b, linearDamper.flange_a);
    connect(linearDamper.flange_b, body.flange_a);
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Rectangle(extent={{-50,90},{50,50}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Line(points={{-20,50},{-20,40},{-35,32},{-5,22},{-35,12},{-5,2},{-20,-6},{-20,-15}}),
        Rectangle(extent={{10,40},{30,10}}, lineColor={0,0,0}),
        Line(points={{20,50},{20,40}}),
        Line(points={{20,25},{20,-15}}),
        Rectangle(extent={{-40,-15},{40,-45}}, lineColor={0,0,0}, fillColor={95,95,95}, fillPattern=FillPattern.Solid),
        Line(points={{0,-45},{0,-55},{-12,-62},{12,-70},{0,-78},{0,-90}}),
        Line(points={{-60,-90},{60,-90}}, thickness=1),
        Text(extent={{-150,145},{150,105}}, textString="%name", textColor={0,0,255})}));
  end Corner;

  model BumpComparison "Linear damper versus hydraulic damper with clean and aerated oil over a 50 mm bump"
    extends Daedalus.Icons.Example;
    RoadBump bump(h=0.05, L=0.5, v=10);
    Corner linear(hydraulicDamper=false, wheel.s(start=0.3, fixed=true), body.s(start=0.6, fixed=true),
      wheel.v(start=0, fixed=true), body.v(start=0, fixed=true));
    Corner clean(x_air=0, wheel.s(start=0.3, fixed=true), body.s(start=0.6, fixed=true),
      wheel.v(start=0, fixed=true), body.v(start=0, fixed=true));
    Corner aerated(x_air=0.02, wheel.s(start=0.3, fixed=true), body.s(start=0.6, fixed=true),
      wheel.v(start=0, fixed=true), body.v(start=0, fixed=true));
  equation
    connect(bump.y, linear.z_road);
    connect(bump.y, clean.z_road);
    connect(bump.y, aerated.z_road);
    annotation (experiment(StopTime=2, Interval=0.001), Documentation(info="<html>
<p>Source: Claytex, <i>A look inside VeSyMA's quarter car models</i>,
<a href=\"https://www.claytex.com/tech-blog/a-look-inside-vesymas-quarter-car-models/\">claytex.com/tech-blog</a>.</p>
<p>Three identical corners cross the same bump: one with an ideal linear damper and two with a physical
monotube damper built from hydraulic components (rebound and compression valves, bleed orifice and a
gas-charged floating piston), once with clean oil and once with 2 % entrained air. The hydraulic damper
gives the asymmetric, digressive force-velocity behaviour of a real damper; aerated oil adds compliance
in series, which shows as a lag and a hysteresis loop in the force-velocity plot
(<code>clean.damper.F_damper</code> against <code>clean.damper.v_damper</code>, and the same for
<code>aerated</code>). Compare <code>*.a_body</code> (comfort) and <code>*.F_tyre</code> (road holding).</p>
<p>Assumed: 400 kg quarter body, 40 kg wheel, 30 kN/m spring, 250 kN/m tyre, 46 mm piston, 20 bar gas charge.</p></html>"));
  end BumpComparison;
end QuarterCar;
