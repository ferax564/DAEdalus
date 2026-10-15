within Daedalus.Examples;
package LandingGear "Oleo-pneumatic landing gear drop test (after Modelon)"
  extends Daedalus.Icons.ExamplesPackage;
  import Tr = Daedalus.Mechanics.Translational;

  model OleoStrut "Oleo-pneumatic shock absorber: polytropic gas spring, orifice damping, seal friction and stops"
    extends Tr.Interfaces.PartialCompliant;
    parameter Units.Length stroke_max=0.40 "Available stroke";
    parameter Units.Area A_gas=3.141592653589793/4*0.15^2 "Pneumatic area (piston)";
    parameter Units.Area A_oil=A_gas "Hydraulic area pumping oil through the orifice";
    parameter Units.Area A_orifice=5e-4 "Main damping orifice area";
    parameter Real Cd=0.7 "Orifice discharge coefficient";
    parameter Units.Volume V_gas0=0.01 "Gas volume fully extended";
    parameter Units.AbsolutePressure p_gas0=3.0e6 "Gas pressure fully extended";
    parameter Real n_poly=1.3 "Polytropic exponent of the gas during the drop";
    parameter Units.Force F_seal=1500 "Seal friction (regularised Coulomb)";
    parameter Units.TranslationalSpringConstant c_stop=5e7;
    parameter Units.TranslationalDampingConstant d_stop=2e5;
    parameter Units.Length L_extended=1.0 "Strut length fully extended";
    Units.Length x "Stroke (compression), 0 = fully extended";
    Units.Velocity x_dot "Closure rate";
    Units.AbsolutePressure p_gas;
    Units.Force F_gas, F_oil, F_friction, F_stops;
    Units.Force F_strut "Total strut force (compression positive)";
    Units.Energy E_absorbed(start=0, fixed=true) "Work done on the strut";
  protected
    constant Units.Density rho=Daedalus.Hydraulics.Oil.rho;
  equation
    x = L_extended - s_rel;
    x_dot = -v_rel;
    p_gas = p_gas0*(V_gas0/(V_gas0 - A_gas*min(max(x, 0), 0.98*V_gas0/A_gas)))^n_poly;
    F_gas = p_gas*A_gas;
    F_oil = rho*A_oil^3/(2*(Cd*A_orifice)^2)*x_dot*abs(x_dot);
    F_friction = F_seal*tanh(x_dot/0.01);
    F_stops = (if x < 0 then c_stop*x + d_stop*x_dot else 0)
            + (if x > stroke_max then c_stop*(x - stroke_max) + d_stop*x_dot else 0);
    F_strut = F_gas + F_oil + F_friction + F_stops;
    f = -F_strut;
    der(E_absorbed) = F_strut*x_dot;
    annotation (Icon(graphics={
        Rectangle(extent={{-80,30},{10,-30}}, lineColor={0,0,0}, fillColor={200,200,200}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{10,16},{90,-16}}, lineColor={0,0,0}, fillColor={160,160,160}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-78,28},{-30,-28}}, lineColor={0,90,190}, fillColor={170,200,240}, fillPattern=FillPattern.Solid),
        Line(points={{-100,0},{-80,0}}, color={0,127,0}), Line(points={{90,0},{100,0}}, color={0,127,0})}));
  end OleoStrut;

  model DropTest "Single drop: sprung mass on oleo strut, unsprung axle, tyre, wing lift equal to weight"
    parameter Units.Velocity v_sink=3.05 "Vertical velocity at touchdown (10 ft/s limit landing)";
    parameter Units.Mass M_sprung=10000 "Aircraft mass carried by this gear";
    parameter Units.Mass m_unsprung=150 "Axle, wheel and brake";
    parameter Real liftFactor=1 "Wing lift as a fraction of weight (CS 25.473: 1.0)";
    parameter Units.Length R_tyre=0.6 "Unloaded tyre radius";
    parameter Units.TranslationalSpringConstant c_tyre=2e6;
    parameter Units.TranslationalDampingConstant d_tyre=4e3;
    parameter Real n_tyre=1.2 "Progressive tyre stiffness exponent";
    OleoStrut strut;
    Tr.Components.Mass sprung(m=M_sprung, g=-9.81, s(start=R_tyre + strut.L_extended + strut.p_gas0*strut.A_gas/strut.c_stop, fixed=true), v(start=-v_sink, fixed=true));
    Tr.Components.Mass axle(m=m_unsprung, g=-9.81, s(start=R_tyre, fixed=true), v(start=-v_sink, fixed=true));
    Tr.Components.ElastoGap tyre(c=c_tyre, d=d_tyre, n=n_tyre, s_rel0=R_tyre);
    Tr.Components.Fixed ground;
    Tr.Sources.ConstantForce lift(f_constant=liftFactor*(M_sprung + m_unsprung)*9.81);
    Real loadFactor(unit="1") "Ground reaction / weight";
    parameter Real y_vis=0 "Lateral position in the 3-D animation" annotation (Dialog(tab="Animation"));
    Daedalus.Visualizers.LandingGear vis(y0=y_vis, R_tyre=R_tyre, z_axle=axle.s, z_top=sprung.s) "3-D animation (draws only)";
    Units.Force F_ground;
    Units.Energy E_kinetic0 = 0.5*(M_sprung + m_unsprung)*v_sink^2;
  equation
    F_ground = -tyre.f;
    loadFactor = F_ground/((M_sprung + m_unsprung)*9.81);
    connect(ground.flange, tyre.flange_a);
    connect(tyre.flange_b, axle.flange_a);
    connect(axle.flange_b, strut.flange_a);
    connect(strut.flange_b, sprung.flange_a);
    connect(lift.flange, sprung.flange_b);
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={95,95,95}, fillColor={250,250,250}, fillPattern=FillPattern.Solid, radius=6),
        Rectangle(extent={{-60,90},{60,60}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-12,60},{12,0}}, lineColor={0,0,0}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-6,10},{6,-30}}, lineColor={0,0,0}, fillColor={160,160,164}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-30,-30},{30,-90}}, lineColor={0,0,0}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
        Line(points={{-80,-92},{80,-92}}, thickness=1),
        Polygon(points={{70,40},{80,40},{75,20},{70,40}}, lineColor={0,0,0}, fillColor={0,0,0}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,145},{150,105}}, textString="%name", textColor={0,0,255})}));
  end DropTest;

  model SinkRates "Drop tests at 2.0, 3.05 (limit) and 3.66 m/s (reserve energy)"
    extends Daedalus.Icons.Example;
    Daedalus.Visualizers.World world "Animation: z up";
    DropTest low(v_sink=2.0, y_vis=2.6);
    DropTest limit(v_sink=3.05);
    DropTest reserve(v_sink=3.66, y_vis=-2.6);
    annotation (experiment(StopTime=1.5, Interval=0.001, Tolerance=1e-7), Documentation(info="<html>
<p>Source: Modelon, <i>Aircraft landing gear design</i>,
<a href=\"https://www.modelon.com/blog/aircraft-landing-gear-design/\">modelon.com/blog</a>.</p>
<p>A drop test in the sense of CS/FAR 25.723: the gear and its share of aircraft mass touch down at
a given sink rate while wing lift balances the weight. The oleo strut absorbs the energy through
the orifice (quadratic in closure rate) and stores some in the gas spring, which also carries the static
load. Plot <code>*.strut.x</code> (stroke), <code>*.loadFactor</code> and the force-stroke curve
<code>limit.strut.F_strut</code> versus <code>limit.strut.x</code> (parametric plot).
Shock absorber efficiency = <code>E_absorbed</code> at maximum stroke divided by
<code>max(F_strut)*max(x)</code>; the defaults give about 0.9 for the limit drop with a load factor near 1.6.</p>
<p>Assumed: 10 t per main gear, 150 mm piston, 0.4 m stroke, 30 bar precharge, 5 cm2 orifice, polytropic exponent 1.3
and a 2 MN/m tyre. The Modelon study uses its own aircraft data; these values are representative of a
narrow-body main gear.</p></html>"));
  end SinkRates;
end LandingGear;
