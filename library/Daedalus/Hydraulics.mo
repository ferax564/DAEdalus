within Daedalus;
package Hydraulics "Lumped-parameter hydraulics: volume-flow ports, compressible oil with entrained air, valves and cylinders"
  extends Daedalus.Icons.Package;

  package Oil "Default mineral oil (ISO VG 46 at 40 degC) and the effective bulk modulus model"
    extends Daedalus.Icons.Package;
    constant Units.Density rho=870 "Density";
    constant Units.DynamicViscosity mu=0.04 "Dynamic viscosity";
    constant Units.BulkModulus beta_oil=1.5e9 "Bulk modulus of the air-free oil";
    constant Units.AbsolutePressure p_atm=101325 "Reference pressure of the entrained air fraction";

    function beta_eff "Effective bulk modulus of oil with entrained (undissolved) air"
      input Units.Pressure p "Absolute pressure";
      input Real x0 "Air volume fraction at p_atm";
      input Real n=1.4 "Polytropic exponent of the air (1: isothermal, 1.4: adiabatic)";
      input Units.BulkModulus beta_l=beta_oil "Bulk modulus of the liquid";
      output Units.BulkModulus beta;
    protected
      Real p_c "Clamped pressure";
      Real v_l "Liquid volume per unit volume at p_atm";
      Real v_g "Gas volume per unit volume at p_atm";
    algorithm
      p_c := max(p, 1000);
      v_l := (1 - x0)*exp(-(p_c - p_atm)/beta_l);
      v_g := x0*(p_atm/p_c)^(1/n);
      beta := (v_l + v_g)/(v_l/beta_l + v_g/(n*p_c));
      annotation (Documentation(info="<html><p>Mixture of liquid and free air: the liquid compresses with
<code>beta_l</code>, the air polytropically. The result is the secant-free tangent bulk modulus
<code>-V dp/dV</code> of the mixture. With <code>x0=0</code> this returns <code>beta_l</code>; with a
few percent of air the stiffness collapses at low pressure, which is what produces the
soft, delayed response seen in actuators and dampers with aerated oil.</p></html>"));
    end beta_eff;
  end Oil;

  package Functions "Regularised flow laws"
    extends Daedalus.Icons.Package;

    function regRoot "sign(x)*sqrt(|x|) with a finite slope at the origin"
      input Real x;
      input Real delta=0.01 "Range around zero where the slope is finite";
      output Real y;
    algorithm
      y := x/(x*x + delta*delta)^0.25;
    end regRoot;

    function orificeFlow "Turbulent orifice flow with laminar transition"
      input Units.PressureDifference dp;
      input Units.Area A "Flow area";
      input Real Cd=0.65 "Discharge coefficient";
      input Units.PressureDifference dp_lam=1e4 "Pressure drop below which flow becomes laminar";
      input Units.Density rho=Oil.rho;
      output Units.VolumeFlowRate q;
    algorithm
      q := Cd*A*sqrt(2/rho)*regRoot(dp, dp_lam);
    end orificeFlow;
  end Functions;

  package Interfaces "Hydraulic ports and partial models"
    extends Daedalus.Icons.Package;

    connector Port "Hydraulic port: absolute pressure and volume flow into the component"
      Units.AbsolutePressure p(start=1e5) "Pressure in the port";
      flow Units.VolumeFlowRate q "Volume flow rate into the component";
    end Port;

    connector Port_a "Hydraulic port (filled)"
      extends Port;
      annotation (Icon(graphics={Ellipse(extent={{-100,100},{100,-100}}, lineColor={0,90,190},
          fillColor={0,90,190}, fillPattern=FillPattern.Solid)}));
    end Port_a;

    connector Port_b "Hydraulic port (open)"
      extends Port;
      annotation (Icon(graphics={Ellipse(extent={{-100,100},{100,-100}}, lineColor={0,90,190},
          fillColor={255,255,255}, fillPattern=FillPattern.Solid)}));
    end Port_b;

    partial model PartialTwoPort "Flow element without storage between two ports"
      extends Daedalus.Icons.Hydraulic;
      Port_a port_a annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Port_b port_b annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Units.PressureDifference dp "port_a.p - port_b.p";
      Units.VolumeFlowRate q "Volume flow from port_a to port_b";
    equation
      dp = port_a.p - port_b.p;
      q = port_a.q;
      port_a.q + port_b.q = 0;
    end PartialTwoPort;
  end Interfaces;

  package Sources "Pressure and flow sources"
    extends Daedalus.Icons.Package;

    model Tank "Reservoir at constant pressure"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.AbsolutePressure p=Oil.p_atm "Tank pressure";
      Interfaces.Port_a port annotation (Placement(transformation(extent={{-10,90},{10,110}})));
    equation
      port.p = p;
      annotation (Icon(graphics={Line(points={{-60,60},{-60,-40},{60,-40},{60,60}}, color={0,90,190}, thickness=0.5),
          Rectangle(extent={{-58,10},{58,-38}}, lineColor={0,90,190}, fillColor={170,200,240}, fillPattern=FillPattern.Solid),
          Line(points={{0,90},{0,10}}, color={0,90,190})}));
    end Tank;

    model PressureSource "Ideal pressure source (supply line)"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.AbsolutePressure p=100e5 "Supply pressure";
      Interfaces.Port_b port annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Units.Power hydraulicPower "Hydraulic power delivered";
    equation
      port.p = p;
      hydraulicPower = -port.p*port.q;
      annotation (Icon(graphics={Ellipse(extent={{-60,60},{60,-60}}, lineColor={0,90,190}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Text(extent={{-40,30},{40,-30}}, textString="P"), Line(points={{60,0},{90,0}}, color={0,90,190})}));
    end PressureSource;

    model PrescribedPressure "Pressure source driven by a signal"
      extends Daedalus.Icons.Hydraulic;
      Daedalus.Blocks.Interfaces.RealInput p(unit="Pa") annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Interfaces.Port_b port annotation (Placement(transformation(extent={{90,-10},{110,10}})));
    equation
      port.p = p;
    end PrescribedPressure;

    model FlowSource "Ideal volume flow source driven by a signal"
      extends Daedalus.Icons.Hydraulic;
      Daedalus.Blocks.Interfaces.RealInput q(unit="m3/s") annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Interfaces.Port_b port annotation (Placement(transformation(extent={{90,-10},{110,10}})));
    equation
      port.q = -q;
    end FlowSource;

    model Pump "Fixed-displacement pump driven by a rotational flange"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.DisplacementPerRevolution D=10e-6/(2*3.141592653589793) "Displacement per radian (10 cc/rev default)";
      parameter Units.Efficiency eta_vol=0.95 "Volumetric efficiency at dp_nom";
      parameter Units.Efficiency eta_mech=0.9 "Hydro-mechanical efficiency";
      parameter Units.PressureDifference dp_nom=200e5 "Pressure difference at which eta_vol is given";
      Interfaces.Port_a port_a "Suction" annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Interfaces.Port_b port_b "Delivery" annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Daedalus.Mechanics.Rotational.Interfaces.Flange_a shaft annotation (Placement(transformation(extent={{-10,-110},{10,-90}})));
      Units.AngularVelocity w;
      Units.PressureDifference dp "Delivery minus suction";
      Units.VolumeFlowRate q "Delivered flow";
    equation
      w = der(shaft.phi);
      dp = port_b.p - port_a.p;
      // linear leakage calibrated so that the volumetric efficiency is eta_vol at dp_nom and 1000 rpm
      q = D*w - (1 - eta_vol)*D*(1000*2*3.141592653589793/60)*dp/dp_nom;
      port_a.q = q;
      port_b.q = -q;
      shaft.tau = D*dp/eta_mech;
      annotation (Icon(graphics={Ellipse(extent={{-60,60},{60,-60}}, lineColor={0,90,190}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Polygon(points={{-20,40},{40,0},{-20,-40},{-20,40}}, fillColor={0,90,190}, fillPattern=FillPattern.Solid),
          Line(points={{-100,0},{-60,0}}, color={0,90,190}), Line(points={{60,0},{100,0}}, color={0,90,190})}));
    end Pump;
  end Sources;

  package Components "Volumes, restrictions, valves, accumulators"
    extends Daedalus.Icons.Package;

    model Volume "Lumped compressible oil volume (one node of the hydraulic network)"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.Volume V=1e-4 "Volume";
      parameter Real x_air=0 "Entrained air volume fraction at atmospheric pressure";
      parameter Units.AbsolutePressure p_start=Oil.p_atm "Initial pressure";
      Interfaces.Port_a port annotation (Placement(transformation(extent={{-10,-110},{10,-90}})));
      Units.AbsolutePressure p(start=p_start, fixed=true, stateSelect=StateSelect.prefer);
      Units.BulkModulus beta;
    equation
      beta = Oil.beta_eff(p, x_air);
      port.p = p;
      V/beta*der(p) = port.q;
      annotation (Icon(graphics={Ellipse(extent={{-70,70},{70,-70}}, lineColor={0,90,190}, fillColor={210,225,245}, fillPattern=FillPattern.Solid),
          Text(extent={{-60,20},{60,-20}}, textString="V=%V")}));
    end Volume;

    model Orifice "Sharp-edged orifice (turbulent, regularised near zero flow)"
      extends Interfaces.PartialTwoPort;
      parameter Units.Area A=1e-6 "Flow area";
      parameter Real Cd=0.65 "Discharge coefficient";
      parameter Units.PressureDifference dp_lam=1e4 "Laminar transition pressure drop";
    equation
      q = Functions.orificeFlow(dp, A, Cd, dp_lam);
      annotation (Icon(graphics={Line(points={{-100,0},{100,0}}, color={0,90,190}),
          Line(points={{-20,40},{0,6},{20,40}}, color={0,90,190}), Line(points={{-20,-40},{0,-6},{20,-40}}, color={0,90,190})}));
    end Orifice;

    model VariableOrifice "Orifice with flow area from an input signal"
      extends Interfaces.PartialTwoPort;
      parameter Real Cd=0.65;
      parameter Units.PressureDifference dp_lam=1e4;
      parameter Units.Area A_leak=1e-10 "Leakage area when closed";
      Daedalus.Blocks.Interfaces.RealInput A(unit="m2") annotation (Placement(transformation(origin={0,110}, extent={{-20,-20},{20,20}}, rotation=270)));
    equation
      q = Functions.orificeFlow(dp, max(A, 0) + A_leak, Cd, dp_lam);
    end VariableOrifice;

    model LaminarRestriction "Linear (laminar) resistance, e.g. a long thin pipe"
      extends Interfaces.PartialTwoPort;
      parameter Real G(unit="m3/(s.Pa)")=1e-11 "Conductance";
    equation
      q = G*dp;
    end LaminarRestriction;

    model Pipe "Laminar pipe resistance from Hagen-Poiseuille with lumped compliance at port_b side"
      extends Interfaces.PartialTwoPort;
      parameter Units.Length L=1 "Length";
      parameter Units.Length d=0.01 "Inner diameter";
    protected
      parameter Real G=3.141592653589793*d^4/(128*Oil.mu*L);
    equation
      q = G*dp;
    end Pipe;

    model CheckValve "Poppet check valve: opens from port_a to port_b above the cracking pressure"
      extends Interfaces.PartialTwoPort;
      parameter Units.Area A_max=1e-5 "Fully open flow area";
      parameter Units.PressureDifference dp_crack=0.5e5 "Cracking pressure";
      parameter Units.PressureDifference dp_open=1e5 "Additional pressure for full opening";
      parameter Real Cd=0.65;
      parameter Units.Area A_leak=1e-11 "Leakage area";
      Real opening(min=0, max=1) "Relative opening";
    equation
      opening = min(1, max(0, (dp - dp_crack)/dp_open));
      q = Functions.orificeFlow(dp, A_max*opening + A_leak, Cd, 1e4);
      annotation (Icon(graphics={Line(points={{-100,0},{-30,0}}, color={0,90,190}), Line(points={{30,0},{100,0}}, color={0,90,190}),
          Ellipse(extent={{-30,20},{10,-20}}, lineColor={0,90,190}), Line(points={{10,30},{10,-30}}, color={0,90,190}, thickness=0.5)}));
    end CheckValve;

    model ReliefValve "Pressure relief valve: limits port_a.p - port_b.p to p_set"
      extends Interfaces.PartialTwoPort;
      parameter Units.PressureDifference p_set=210e5 "Setting (cracking) pressure";
      parameter Units.PressureDifference p_override=10e5 "Pressure override to full opening";
      parameter Units.Area A_max=2e-5;
      parameter Real Cd=0.65;
      Real opening(min=0, max=1);
    equation
      opening = min(1, max(0, (dp - p_set)/p_override));
      q = Functions.orificeFlow(dp, A_max*opening + 1e-12, Cd, 1e4);
      annotation (Icon(graphics={Rectangle(extent={{-40,40},{40,-40}}, lineColor={0,90,190}),
          Line(points={{-30,-20},{30,20}}, color={0,90,190}, arrow={Arrow.None,Arrow.Filled}),
          Line(points={{-100,0},{-40,0}}, color={0,90,190}), Line(points={{40,0},{100,0}}, color={0,90,190})}));
    end ReliefValve;

    model Accumulator "Gas-charged accumulator (polytropic gas, bladder separates gas and oil)"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.Volume V0=1e-3 "Total (gas) volume";
      parameter Units.AbsolutePressure p0=50e5 "Gas precharge pressure";
      parameter Real n=1.4 "Polytropic exponent";
      parameter Units.Volume V_dead=1e-6 "Oil volume remaining in the port when empty";
      parameter Units.AbsolutePressure p_start=p0;
      Interfaces.Port_a port annotation (Placement(transformation(extent={{-10,-110},{10,-90}})));
      Units.AbsolutePressure p(start=p_start, fixed=true, stateSelect=StateSelect.prefer);
      Units.Volume V_oil "Oil volume stored";
      Real C(unit="m3/Pa") "Hydraulic capacitance dV_oil/dp";
    equation
      port.p = p;
      V_oil = if p > p0 then V0*(1 - (p0/p)^(1/n)) else 0;
      C = V_dead/Oil.beta_oil + (if p > p0 then V0/n*(p0/p)^(1/n)/p else 0);
      C*der(p) = port.q;
      annotation (Icon(graphics={Ellipse(extent={{-50,90},{50,-80}}, lineColor={0,90,190}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-50,0},{50,0}}, color={0,90,190}), Line(points={{0,-80},{0,-100}}, color={0,90,190})}));
    end Accumulator;

    model ServoValve4_3 "4/3 servo valve (critical centre) with second-order spool dynamics"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.VolumeFlowRate q_nom=10/60000 "Rated flow at full spool stroke (10 l/min)";
      parameter Units.PressureDifference dp_nom=70e5 "Rated valve pressure drop (both metering edges)";
      parameter Units.Frequency f_spool=80 "Spool natural frequency";
      parameter Real D_spool=0.7 "Spool damping ratio";
      parameter Real leak=0.002 "Leakage as fraction of rated opening at centre";
      Interfaces.Port_a P "Supply" annotation (Placement(transformation(extent={{-50,-110},{-30,-90}})));
      Interfaces.Port_b T "Return" annotation (Placement(transformation(extent={{30,-110},{50,-90}})));
      Interfaces.Port_b A annotation (Placement(transformation(extent={{-50,90},{-30,110}})));
      Interfaces.Port_b B annotation (Placement(transformation(extent={{30,90},{50,110}})));
      Daedalus.Blocks.Interfaces.RealInput u "Normalised spool command (-1..1)"
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Real x(start=0, fixed=true) "Normalised spool position";
      Real xd(start=0, fixed=true) "Spool velocity";
      Units.VolumeFlowRate q_PA, q_BT, q_PB, q_AT;
    protected
      parameter Real w=2*3.141592653589793*f_spool;
      parameter Real k=q_nom/sqrt(dp_nom/2) "Flow gain per metering edge";
      parameter Units.PressureDifference dp_lam=1e5;
      Real xc "Clamped spool position";
    equation
      der(x) = xd;
      der(xd) = w*w*(min(1, max(-1, u)) - x) - 2*D_spool*w*xd;
      xc = min(1, max(-1, x));
      q_PA = k*(max(xc, 0) + leak)*Functions.regRoot(P.p - A.p, dp_lam);
      q_BT = k*(max(xc, 0) + leak)*Functions.regRoot(B.p - T.p, dp_lam);
      q_PB = k*(max(-xc, 0) + leak)*Functions.regRoot(P.p - B.p, dp_lam);
      q_AT = k*(max(-xc, 0) + leak)*Functions.regRoot(A.p - T.p, dp_lam);
      P.q = q_PA + q_PB;
      T.q = -q_BT - q_AT;
      A.q = -q_PA + q_AT;
      B.q = -q_PB + q_BT;
      annotation (Icon(graphics={
          Rectangle(extent={{-90,40},{-30,-40}}, lineColor={0,90,190}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{-30,40},{30,-40}}, lineColor={0,90,190}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{30,40},{90,-40}}, lineColor={0,90,190}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-80,-30},{-40,30}}, color={0,90,190}, arrow={Arrow.None,Arrow.Filled}),
          Line(points={{-40,-30},{-80,30}}, color={0,90,190}, arrow={Arrow.None,Arrow.Filled}),
          Line(points={{40,-30},{40,30}}, color={0,90,190}, arrow={Arrow.None,Arrow.Filled}),
          Line(points={{80,30},{80,-30}}, color={0,90,190}, arrow={Arrow.None,Arrow.Filled}),
          Text(extent={{-150,-50},{150,-80}}, textString="%name", textColor={0,0,255})}));
    end ServoValve4_3;
  end Components;

  package Actuators "Hydraulic cylinders"
    extends Daedalus.Icons.Package;

    model Cylinder "Double-acting cylinder (single or through rod) with chamber compressibility, friction and end stops"
      extends Daedalus.Icons.Hydraulic;
      parameter Units.Length D_piston=0.05 "Piston (bore) diameter";
      parameter Units.Length d_rod_A=0 "Rod diameter on the A side (0: no rod; >0 with d_rod_B: through rod)";
      parameter Units.Length d_rod_B=0.025 "Rod diameter on the B side";
      parameter Units.Length stroke=0.2 "Stroke";
      parameter Units.Volume V_dead=1e-5 "Dead volume of each chamber (incl. hoses)";
      parameter Real x_air=0 "Entrained air volume fraction at atmospheric pressure";
      parameter Units.TranslationalDampingConstant d_visc=1000 "Viscous friction";
      parameter Units.Force F_coulomb=50 "Coulomb friction (regularised)";
      parameter Units.TranslationalSpringConstant c_stop=1e8 "End-stop stiffness";
      parameter Units.TranslationalDampingConstant d_stop=1e5 "End-stop damping";
      parameter Units.Length s_start=stroke/2 "Initial extension";
      parameter Units.AbsolutePressure pA_start=Oil.p_atm;
      parameter Units.AbsolutePressure pB_start=Oil.p_atm;
      Interfaces.Port_a port_A "Chamber A (extends the rod when pressurised)"
        annotation (Placement(transformation(extent={{-70,50},{-50,70}})));
      Interfaces.Port_b port_B "Chamber B" annotation (Placement(transformation(extent={{50,50},{70,70}})));
      Daedalus.Mechanics.Translational.Interfaces.Flange_a housing annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Daedalus.Mechanics.Translational.Interfaces.Flange_b rod annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Units.Position s(start=s_start, fixed=true, stateSelect=StateSelect.prefer) "Extension (rod.s - housing.s)";
      Units.Velocity v(start=0, fixed=true, stateSelect=StateSelect.prefer) "Extension speed";
      Units.AbsolutePressure pA(start=pA_start, fixed=true, stateSelect=StateSelect.prefer);
      Units.AbsolutePressure pB(start=pB_start, fixed=true, stateSelect=StateSelect.prefer);
      Units.Volume VA, VB;
      Units.Force F_hyd "Hydraulic force extending the rod";
      Units.Force F_fric "Friction force opposing extension";
      Units.Force F_stop "End-stop force opposing extension";
      Units.Force f "Tension between housing and rod (= rod.f)";
    protected
      parameter Units.Area AA=3.141592653589793/4*(D_piston^2 - d_rod_A^2);
      parameter Units.Area AB=3.141592653589793/4*(D_piston^2 - d_rod_B^2);
    equation
      s = rod.s - housing.s;
      v = der(s);
      VA = V_dead + AA*max(s, 0);
      VB = V_dead + AB*max(stroke - s, 0);
      VA/Oil.beta_eff(pA, x_air)*der(pA) = port_A.q - AA*v;
      VB/Oil.beta_eff(pB, x_air)*der(pB) = port_B.q + AB*v;
      port_A.p = pA;
      port_B.p = pB;
      F_hyd = pA*AA - pB*AB;
      F_fric = d_visc*v + F_coulomb*tanh(v/1e-3);
      F_stop = if s > stroke then c_stop*(s - stroke) + d_stop*v
               else if s < 0 then c_stop*s + d_stop*v else 0;
      f = F_fric + F_stop - F_hyd;
      rod.f = f;
      housing.f = -f;
      annotation (Icon(graphics={
          Rectangle(extent={{-90,40},{40,-40}}, lineColor={0,90,190}, fillColor={235,240,250}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{-20,38},{-6,-38}}, fillColor={95,95,95}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{-6,8},{100,-8}}, fillColor={160,160,160}, fillPattern=FillPattern.Solid),
          Line(points={{-60,50},{-60,40}}, color={0,90,190}), Line(points={{30,50},{30,40}}, color={0,90,190}),
          Text(extent={{-150,-50},{150,-90}}, textString="%name", textColor={0,0,255})}));
    end Cylinder;
  end Actuators;

  package Sensors "Hydraulic sensors"
    extends Daedalus.Icons.Package;

    model PressureSensor "Absolute pressure"
      Interfaces.Port_a port annotation (Placement(transformation(extent={{-10,-110},{10,-90}})));
      Daedalus.Blocks.Interfaces.RealOutput p(unit="Pa") annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    equation
      p = port.p;
      port.q = 0;
    end PressureSensor;

    model FlowSensor "Volume flow from port_a to port_b"
      extends Interfaces.PartialTwoPort;
      Daedalus.Blocks.Interfaces.RealOutput q_out(unit="m3/s")
        annotation (Placement(transformation(origin={0,-110}, extent={{-10,-10},{10,10}}, rotation=270)));
    equation
      dp = 0;
      q_out = q;
    end FlowSensor;
  end Sensors;
end Hydraulics;
