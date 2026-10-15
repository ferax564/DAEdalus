within Daedalus.Examples;
package AileronActuator "Electro-hydraulic aileron actuator (after Modelon)"
  extends Daedalus.Icons.ExamplesPackage;
  import H = Daedalus.Hydraulics;
  import Tr = Daedalus.Mechanics.Translational;
  import Rot = Daedalus.Mechanics.Rotational;

  model HydraulicSupply "Aircraft hydraulic system: 3000 psi supply and 50 psi return"
    parameter Units.AbsolutePressure p_supply=207e5 "3000 psi";
    parameter Units.AbsolutePressure p_return=3.5e5 "50 psi";
    H.Interfaces.Port_b P annotation (Placement(transformation(extent={{-50,90},{-30,110}})));
    H.Interfaces.Port_a T annotation (Placement(transformation(extent={{30,90},{50,110}})));
    H.Sources.PressureSource supply(p=p_supply);
    H.Sources.Tank ret(p=p_return);
    H.Components.Pipe supplyLine(L=3, d=0.008);
    H.Components.Pipe returnLine(L=3, d=0.008);
  equation
    connect(supply.port, supplyLine.port_a);
    connect(supplyLine.port_b, P);
    connect(T, returnLine.port_a);
    connect(returnLine.port_b, ret.port);
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Ellipse(extent={{-60,40},{20,-40}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Polygon(points={{-20,40},{0,20},{-40,20},{-20,40}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
        Line(points={{-40,90},{-40,40}}, color={0,128,255}, thickness=1),
        Line(points={{40,90},{40,-60},{-20,-60},{-20,-40}}, color={0,128,255}, thickness=1),
        Text(extent={{-90,-70},{90,-95}}, textString="3000 psi", textColor={64,64,64}),
        Text(extent={{-150,145},{150,105}}, textString="%name", textColor={0,0,255})}));
  end HydraulicSupply;

  model Actuator "Servo valve driving a balanced (through-rod) cylinder"
    parameter Units.Length D_piston=0.040;
    parameter Units.Length d_rod=0.020;
    parameter Units.Length stroke=0.060;
    parameter Real x_air=0.001 "Entrained air in the actuator chambers";
    H.Components.ServoValve4_3 valve(q_nom=20/60000, dp_nom=70e5, f_spool=100, D_spool=0.7);
    H.Actuators.Cylinder cylinder(D_piston=D_piston, d_rod_A=d_rod, d_rod_B=d_rod, stroke=stroke,
      s_start=stroke/2, V_dead=2e-5, x_air=x_air, pA_start=105e5, pB_start=105e5, d_visc=2000, F_coulomb=200);
    H.Interfaces.Port_a P annotation (Placement(transformation(extent={{-50,90},{-30,110}})));
    H.Interfaces.Port_b T annotation (Placement(transformation(extent={{30,90},{50,110}})));
    Tr.Interfaces.Flange_a mount annotation (Placement(transformation(extent={{-110,-20},{-90,0}})));
    Tr.Interfaces.Flange_b rod annotation (Placement(transformation(extent={{90,-20},{110,0}})));
    Daedalus.Blocks.Interfaces.RealInput u "Normalised valve command";
  equation
    connect(P, valve.P);
    connect(valve.T, T);
    connect(valve.A, cylinder.port_A);
    connect(valve.B, cylinder.port_B);
    connect(mount, cylinder.housing);
    connect(cylinder.rod, rod);
    connect(u, valve.u);
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Rectangle(extent={{-70,20},{50,-40}}, lineColor={0,0,0}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-20,20},{-10,-40}}, lineColor={0,0,0}, fillColor={95,95,95}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-10,-6},{100,-14}}, lineColor={0,0,0}, fillColor={160,160,164}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-50,70},{30,40}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Line(points={{-40,90},{-40,70}}, color={0,128,255}),
        Line(points={{40,90},{40,70},{30,70}}, color={0,128,255}),
        Line(points={{-30,40},{-30,20}}, color={0,128,255}),
        Line(points={{10,40},{10,20}}, color={0,128,255}),
        Text(extent={{-150,145},{150,105}}, textString="%name", textColor={0,0,255})}));
  end Actuator;

  model PositionControl "Aileron deflection steps and a sine sweep under aerodynamic hinge moment"
    extends Daedalus.Icons.Example;
    parameter Units.Length r_horn=0.08 "Control horn lever arm";
    parameter Real K_hinge(unit="N.m/rad")=5e3 "Aerodynamic hinge-moment stiffness";
    parameter Real Kp=40 "Position loop gain [1/m]";
    HydraulicSupply hyd;
    Actuator act;
    Tr.Components.Fixed wingSpar(s0=-act.stroke/2) "Mount placed so that the rod is at 0 with the actuator at mid-stroke";
    Rot.Components.IdealLever horn(r=r_horn);
    Rot.Components.Inertia aileron(J=1.5) "Aileron about its hinge line";
    Rot.Components.SpringDamper hingeMoment(c=K_hinge, d=50) "Aerodynamic hinge moment (linearised)";
    Rot.Components.Fixed hinge;
    Tr.Sensors.PositionSensor stroke;
    Daedalus.Blocks.Sources.TimeTable command(table=[0, 0; 0.2, 0; 0.2, 10; 1.0, 10; 1.0, -10; 1.8, -10; 1.8, 0; 2.5, 0])
      "Aileron command [deg]";
    Real delta_deg = aileron.phi*180/3.141592653589793 "Aileron deflection";
    Real s_ref "Rod reference from the commanded angle";
    Real e "Position error";
    Daedalus.Visualizers.World world "Animation: z up";
    Daedalus.Visualizers.Aileron vis(phi=aileron.phi, r_horn=r_horn) "3-D animation (draws only)";
  equation
    s_ref = r_horn*command.y*3.141592653589793/180;
    e = s_ref - stroke.s;
    act.u = Kp*e;
    connect(hyd.P, act.P);
    connect(act.T, hyd.T);
    connect(wingSpar.flange, act.mount);
    connect(act.rod, stroke.flange);
    connect(act.rod, horn.flange_b);
    connect(aileron.flange_a, horn.flange_a);
    connect(hingeMoment.flange_a, hinge.flange);
    connect(hingeMoment.flange_b, aileron.flange_b);
    annotation (experiment(StopTime=2.5, Interval=0.001), Documentation(info="<html>
<p>Source: Modelon, <i>Modeling and simulation of aircraft aileron actuator</i>,
<a href=\"https://www.modelon.com/blog/modeling-and-simulation-of-aircraft-aileron-actuator/\">modelon.com/blog</a>.</p>
<p>An aircraft hydraulic supply (3000 psi, 50 psi return, 3 m lines) feeds a flow-control servo
valve and a balanced through-rod cylinder that moves the aileron through a control horn. A
proportional position loop closes around the rod position. The aerodynamic hinge moment is a
linear spring about the hinge line. Plot <code>delta_deg</code> against <code>command.y</code>,
the chamber pressures <code>act.cylinder.pA</code>/<code>pB</code> and the spool <code>act.valve.x</code>.</p>
<p>Assumed: 40 mm bore, 20 mm rods, 60 mm stroke, 20 l/min valve at 70 bar, 100 Hz spool, 80 mm horn.</p></html>"));
  end PositionControl;

  model SpeedVsLoad "No-load to near-stall: rod speed at full valve opening against an increasing opposing load"
    extends Daedalus.Icons.Example;
    HydraulicSupply hyd;
    Actuator act(stroke=10, cylinder(s_start=0.01)) "Long stroke so the rod never reaches an end stop";
    Tr.Components.Fixed mount;
    Tr.Components.Mass load(m=5);
    Tr.Sources.Force opposingForce;
    Daedalus.Blocks.Sources.Ramp F_load(height=-0.95*F_stall, duration=20, startTime=0.5) "Opposing load";
    Daedalus.Blocks.Sources.Constant fullOpen(k=1);
    Units.Force F_applied = -F_load.y "Opposing load magnitude";
    Units.Velocity v_rod = load.v;
    Units.Velocity v_theory "Ideal orifice-flow prediction v0*sqrt(1 - F/F_stall)";
    parameter Units.Area A_piston = 3.141592653589793/4*(act.D_piston^2 - act.d_rod^2);
    parameter Units.Force F_stall = (hyd.p_supply - hyd.p_return)*A_piston "Stall force";
    parameter Units.Velocity v0 = act.valve.q_nom*sqrt((hyd.p_supply - hyd.p_return)/act.valve.dp_nom)/A_piston
      "No-load speed of an ideal valve";
    Daedalus.Visualizers.World world "Animation: z up";
    Daedalus.Visualizers.Shape barrel(shapeType="cylinder", r={-10.25, 0, 0.3}, length=10.25, width=0.11, height=0.11,
      color={30, 90, 170}) "3-D animation: cylinder barrel";
    Daedalus.Visualizers.Shape rod(shapeType="cylinder", r={act.cylinder.s - 10.2, 0, 0.3}, length=10.2, width=0.045,
      height=0.045, color={215, 220, 228}, specularCoefficient=1) "Piston rod";
    Daedalus.Visualizers.Shape mass(r={act.cylinder.s, 0, 0.3}, r_shape={0, 0, -0.15}, length=0.3, width=0.3, height=0.3,
      color={200, 40, 40}) "The load";
    Daedalus.Visualizers.Shape base(r={-10.6, 0, 0}, length=21.4, width=0.8, height=0.04, color={150, 155, 165},
      specularCoefficient=0.2) "Test bench";
  equation
    v_theory = v0*sqrt(max(0, 1 - F_applied/F_stall));
    connect(hyd.P, act.P);
    connect(act.T, hyd.T);
    connect(mount.flange, act.mount);
    connect(act.rod, load.flange_a);
    connect(load.flange_b, opposingForce.flange);
    connect(F_load.y, opposingForce.f);
    connect(fullOpen.y, act.u);
    annotation (experiment(StopTime=21, Interval=0.01), Documentation(info="<html>
<p>Source: Modelon, <i>Modeling and simulation of aircraft aileron actuator</i>,
<a href=\"https://www.modelon.com/blog/modeling-and-simulation-of-aircraft-aileron-actuator/\">modelon.com/blog</a>.</p>
<p>The speed-versus-load curve of a valve-controlled actuator: the valve is held fully open while
the opposing force ramps slowly from zero to 95 % of the stall force, so each instant is a quasi-steady
operating point. Plot <code>v_rod</code> and <code>v_theory</code> against <code>F_applied</code>
(parametric plot). The simulated curve sits below the ideal square-root law because of the line losses,
friction and the laminar region of the metering edges.</p></html>"));
  end SpeedVsLoad;
end AileronActuator;
