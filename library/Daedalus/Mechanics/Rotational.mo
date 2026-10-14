within Daedalus.Mechanics;
package Rotational "1D rotational mechanics: inertias, compliances, gears, clutches, torque converters"
  extends Daedalus.Icons.Package;

  package Interfaces "Rotational flanges and partial models"
    extends Daedalus.Icons.Package;

    connector Flange "Rotational flange (absolute angle and cut torque)"
      Units.Angle phi "Absolute rotation angle of flange";
      flow Units.Torque tau "Cut torque in the flange";
    end Flange;

    connector Flange_a "Rotational flange (filled)"
      extends Flange;
      annotation (Icon(graphics={Ellipse(extent={{-100,100},{100,-100}}, lineColor={0,0,0},
          fillColor={95,95,95}, fillPattern=FillPattern.Solid)}));
    end Flange_a;

    connector Flange_b "Rotational flange (open)"
      extends Flange;
      annotation (Icon(graphics={Ellipse(extent={{-98,100},{102,-100}}, lineColor={0,0,0},
          fillColor={255,255,255}, fillPattern=FillPattern.Solid)}));
    end Flange_b;

    partial model PartialTwoFlanges "Two rotational flanges"
      Flange_a flange_a "Driving flange" annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Flange_b flange_b "Driven flange" annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      annotation (Icon(graphics={Text(extent={{-150,90},{150,50}}, textString="%name", textColor={0,0,255})}));
    end PartialTwoFlanges;

    partial model PartialCompliant "Compliant connection of two flanges"
      extends PartialTwoFlanges;
      Units.Angle phi_rel(start=0) "Relative rotation angle (flange_b.phi - flange_a.phi)";
      Units.AngularVelocity w_rel(start=0) "Relative angular velocity";
      Units.Torque tau "Torque between flanges (= flange_b.tau)";
    equation
      phi_rel = flange_b.phi - flange_a.phi;
      w_rel = der(phi_rel);
      flange_b.tau = tau;
      flange_a.tau = -tau;
    end PartialCompliant;

    partial model PartialElementaryOneFlange "One flange"
      Flange_b flange annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      annotation (Icon(graphics={Text(extent={{-150,140},{150,100}}, textString="%name", textColor={0,0,255})}));
    end PartialElementaryOneFlange;
  end Interfaces;

  package Components "Rotational components"
    extends Daedalus.Icons.Package;

    model Fixed "Flange fixed in housing at a given angle"
      parameter Units.Angle phi0=0 "Fixed offset angle of housing";
      Interfaces.Flange_b flange annotation (Placement(transformation(extent={{-10,-10},{10,10}})));
    equation
      flange.phi = phi0;
      annotation (Icon(graphics={Line(points={{-80,-40},{80,-40}}), Line(points={{-80,-40},{-60,-60}}),
          Line(points={{-40,-40},{-20,-60}}), Line(points={{0,-40},{20,-60}}), Line(points={{40,-40},{60,-60}}),
          Line(points={{0,-40},{0,-10}})}));
    end Fixed;

    model Inertia "1D rotational component with inertia"
      parameter Units.Inertia J(min=0, start=1) "Moment of inertia";
      parameter StateSelect stateSelect=StateSelect.default "Priority to use phi and w as states";
      Interfaces.Flange_a flange_a annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Interfaces.Flange_b flange_b annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Units.Angle phi(stateSelect=stateSelect) "Absolute rotation angle";
      Units.AngularVelocity w(stateSelect=stateSelect) "Absolute angular velocity";
      Units.AngularAcceleration a "Absolute angular acceleration";
    equation
      phi = flange_a.phi;
      phi = flange_b.phi;
      w = der(phi);
      a = der(w);
      J*a = flange_a.tau + flange_b.tau;
      annotation (Icon(graphics={
          Rectangle(extent={{-100,10},{-50,-10}}, lineColor={0,0,0}, fillPattern=FillPattern.HorizontalCylinder, fillColor={192,192,192}),
          Rectangle(extent={{50,10},{100,-10}}, lineColor={0,0,0}, fillPattern=FillPattern.HorizontalCylinder, fillColor={192,192,192}),
          Rectangle(extent={{-50,70},{50,-70}}, lineColor={0,0,0}, fillPattern=FillPattern.HorizontalCylinder, fillColor={128,128,128}, radius=10),
          Text(extent={{-150,120},{150,80}}, textString="%name", textColor={0,0,255}),
          Text(extent={{-150,-80},{150,-120}}, textString="J=%J")}));
    end Inertia;

    model Spring "Linear 1D rotational spring"
      extends Interfaces.PartialCompliant;
      parameter Units.RotationalSpringConstant c(min=0, start=1e5) "Spring constant";
      parameter Units.Angle phi_rel0=0 "Unstretched spring angle";
    equation
      tau = c*(phi_rel - phi_rel0);
      annotation (Icon(graphics={Line(points={{-100,0},{-58,0},{-43,-30},{-13,30},{17,-30},{47,30},{62,0},{100,0}})}));
    end Spring;

    model Damper "Linear 1D rotational damper"
      extends Interfaces.PartialCompliant;
      parameter Units.RotationalDampingConstant d(min=0, start=0) "Damping constant";
      Units.Power lossPower "Power dissipated";
    equation
      tau = d*w_rel;
      lossPower = tau*w_rel;
      annotation (Icon(graphics={Line(points={{-100,0},{-60,0}}), Line(points={{-60,-30},{-60,30}}),
          Line(points={{-60,-30},{60,-30}}), Line(points={{-60,30},{60,30}}),
          Rectangle(extent={{-60,30},{30,-30}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
          Line(points={{30,0},{100,0}})}));
    end Damper;

    model SpringDamper "Linear 1D rotational spring and damper in parallel"
      extends Interfaces.PartialCompliant;
      parameter Units.RotationalSpringConstant c(min=0, start=1e5);
      parameter Units.RotationalDampingConstant d(min=0, start=0);
      parameter Units.Angle phi_rel0=0;
      Units.Power lossPower;
    equation
      tau = c*(phi_rel - phi_rel0) + d*w_rel;
      lossPower = d*w_rel*w_rel;
      annotation (Icon(graphics={Line(points={{-80,32},{-58,32},{-43,2},{-13,62},{17,2},{47,62},{62,32},{80,32}}),
          Line(points={{-80,-40},{-20,-40}}), Rectangle(extent={{-20,-20},{40,-60}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
          Line(points={{40,-40},{80,-40}}), Line(points={{-100,0},{-80,0},{-80,32}}), Line(points={{-80,0},{-80,-40}}),
          Line(points={{100,0},{80,0},{80,32}}), Line(points={{80,0},{80,-40}})}));
    end SpringDamper;

    model ElastoBacklash "Backlash with elastic contact (no tension in contact)"
      extends Interfaces.PartialCompliant;
      parameter Units.RotationalSpringConstant c(min=0, start=1e5) "Contact stiffness";
      parameter Units.RotationalDampingConstant d(min=0, start=0) "Contact damping";
      parameter Units.Angle b(min=0)=0 "Total backlash";
      parameter Units.Angle phi_rel0=0;
    protected
      Units.Angle phi_diff = phi_rel - phi_rel0;
      Units.Torque tau_c;
      Units.Torque tau_d;
    equation
      tau_c = if phi_diff > b/2 then c*(phi_diff - b/2) else if phi_diff < -b/2 then c*(phi_diff + b/2) else 0;
      tau_d = if phi_diff > b/2 or phi_diff < -b/2 then d*w_rel else 0;
      // the damping torque may not pull the contact surfaces together
      tau = if phi_diff > b/2 then max(tau_c + tau_d, 0) else if phi_diff < -b/2 then min(tau_c + tau_d, 0) else 0;
      annotation (Icon(graphics={Line(points={{-100,0},{-40,0},{-40,40},{-10,40}}), Line(points={{-40,0},{-40,-40},{-10,-40}}),
          Line(points={{100,0},{40,0}}), Rectangle(extent={{-4,50},{40,-50}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid)}));
    end ElastoBacklash;

    model IdealGear "Ideal gear without inertia: flange_a.phi = ratio*flange_b.phi"
      extends Interfaces.PartialTwoFlanges;
      parameter Real ratio(start=1) "Transmission ratio (flange_a.phi/flange_b.phi)";
      parameter Real eta(min=0, max=1)=1 "Mechanical efficiency (applied in the power-flow direction)";
    equation
      flange_a.phi = ratio*flange_b.phi;
      0 = ratio*flange_a.tau*(if flange_a.tau*der(flange_a.phi) >= 0 then eta else 1/eta) + flange_b.tau;
      annotation (Icon(graphics={
          Rectangle(extent={{-40,60},{0,-60}}, lineColor={0,0,0}, fillPattern=FillPattern.HorizontalCylinder, fillColor={192,192,192}),
          Rectangle(extent={{0,30},{40,-30}}, lineColor={0,0,0}, fillPattern=FillPattern.HorizontalCylinder, fillColor={192,192,192}),
          Line(points={{-100,0},{-40,0}}), Line(points={{40,0},{100,0}}),
          Text(extent={{-150,-70},{150,-110}}, textString="ratio=%ratio")}));
    end IdealGear;

    model Clutch "Friction clutch with regularised Coulomb friction (normalised normal force input)"
      extends Interfaces.PartialCompliant;
      parameter Real mu=0.3 "Friction coefficient";
      parameter Units.Force fn_max(min=0)=1e4 "Maximum normal force";
      parameter Real cgeo(min=0)=0.1 "Geometry constant (effective radius * number of surfaces) [m]";
      parameter Units.AngularVelocity w_small(min=1e-9)=0.1 "Slip speed for full friction";
      parameter Units.Torque tau_drag=0 "Viscous-like drag torque at w_small when open";
      Daedalus.Blocks.Interfaces.RealInput f_normalized "Normalised normal force (0..1)"
        annotation (Placement(transformation(origin={0,110}, extent={{-20,-20},{20,20}}, rotation=270)));
      Units.Torque tau_max "Maximum transmittable friction torque";
      Units.Power lossPower "Friction power";
    equation
      tau_max = mu*cgeo*fn_max*min(1, max(0, f_normalized));
      tau = (tau_max + tau_drag)*tanh(w_rel/w_small);
      lossPower = tau*w_rel;
      annotation (Icon(graphics={
          Rectangle(extent={{-100,10},{-30,-10}}, fillColor={192,192,192}, fillPattern=FillPattern.HorizontalCylinder),
          Rectangle(extent={{30,10},{100,-10}}, fillColor={192,192,192}, fillPattern=FillPattern.HorizontalCylinder),
          Rectangle(extent={{-30,60},{-10,-60}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{10,60},{30,-60}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
          Line(points={{0,90},{0,60}}, color={0,0,127})}));
    end Clutch;

    model TorqueConverter "Hydrodynamic torque converter from capacity (K-factor) and torque-ratio tables"
      extends Interfaces.PartialTwoFlanges;
      parameter Real table[:, 3]=[0.0, 145, 2.1; 0.2, 147, 1.85; 0.4, 152, 1.58; 0.6, 162, 1.32; 0.7, 170, 1.18;
        0.8, 185, 1.05; 0.85, 200, 1.0; 0.9, 230, 1.0; 0.95, 320, 1.0; 0.98, 600, 1.0; 1.0, 2000, 1.0]
        "Columns: speed ratio w_turbine/w_impeller, K-factor [rpm/sqrt(N.m)], torque ratio";
      parameter Boolean dynamic=false "true: first-order fluid dynamics on the impeller torque";
      parameter Units.Time T_fluid=0.05 "Time constant of the fluid torque build-up (dynamic=true)";
      parameter Units.AngularVelocity w_small=1 "Impeller speed below which torque is regularised";
      constant Real rpm=60/(2*3.141592653589793);
      Units.AngularVelocity w_impeller "Impeller (pump) speed";
      Units.AngularVelocity w_turbine "Turbine speed";
      Real speedRatio "w_turbine / w_impeller (clamped)";
      Real K "Capacity factor [rpm/sqrt(N.m)]";
      Real torqueRatio "Turbine torque / impeller torque";
      Units.Torque tau_impeller "Torque absorbed by the impeller";
      Units.Torque tau_turbine "Torque delivered to the turbine";
      Units.Torque tau_static "Quasi-static impeller torque from the K-factor";
      Units.Power lossPower;
    equation
      w_impeller = der(flange_a.phi);
      w_turbine = der(flange_b.phi);
      speedRatio = min(1, max(0, w_turbine/max(abs(w_impeller), w_small)));
      K = Daedalus.Blocks.Functions.interpolate(table, speedRatio, 2);
      torqueRatio = Daedalus.Blocks.Functions.interpolate(table, speedRatio, 3);
      tau_static = (w_impeller*rpm/K)^2*tanh(w_impeller/w_small);
      if dynamic then
        der(tau_impeller) = (tau_static - tau_impeller)/T_fluid;
      else
        tau_impeller = tau_static;
      end if;
      tau_turbine = torqueRatio*tau_impeller;
      flange_a.tau = tau_impeller;
      flange_b.tau = -tau_turbine;
      lossPower = tau_impeller*w_impeller - tau_turbine*w_turbine;
      annotation (Icon(graphics={
          Ellipse(extent={{-60,60},{60,-60}}, lineColor={0,0,0}, fillColor={200,215,235}, fillPattern=FillPattern.Solid),
          Polygon(points={{-40,30},{0,0},{-40,-30},{-40,30}}, fillColor={95,95,95}, fillPattern=FillPattern.Solid),
          Polygon(points={{40,30},{0,0},{40,-30},{40,30}}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Line(points={{-100,0},{-60,0}}), Line(points={{60,0},{100,0}})}));
    end TorqueConverter;
  end Components;

  package Sources "Torque and speed sources"
    extends Daedalus.Icons.Package;

    model Torque "Input signal acting as external torque on a flange"
      extends Interfaces.PartialElementaryOneFlange;
      Daedalus.Blocks.Interfaces.RealInput tau(unit="N.m") "Accelerating torque"
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    equation
      flange.tau = -tau;
      annotation (Icon(graphics={Line(points={{-96,0},{-40,0}}, color={0,0,127}),
          Line(points={{-30,60},{30,60},{60,0},{30,-60},{-30,-60}}, smooth=Smooth.Bezier, arrow={Arrow.None,Arrow.Filled})}));
    end Torque;

    model ConstantTorque "Constant torque, not dependent on speed"
      extends Interfaces.PartialElementaryOneFlange;
      parameter Units.Torque tau_constant "Constant torque (if negative, torque is acting as load)";
      Units.AngularVelocity w;
    equation
      w = der(flange.phi);
      flange.tau = -tau_constant;
    end ConstantTorque;

    model QuadraticSpeedDependentTorque "Load torque proportional to speed squared"
      extends Interfaces.PartialElementaryOneFlange;
      parameter Units.Torque tau_nominal "Nominal torque (if negative, torque is acting as load)";
      parameter Units.AngularVelocity w_nominal(min=1e-9) "Speed at nominal torque";
      Units.AngularVelocity w;
    equation
      w = der(flange.phi);
      flange.tau = -tau_nominal*(w/w_nominal)^2*sign(w);
    end QuadraticSpeedDependentTorque;

    model Speed "Forced movement of a flange to follow a reference speed"
      extends Interfaces.PartialElementaryOneFlange;
      parameter Units.Time T(min=1e-9)=0.01 "Time constant of the speed follower";
      Daedalus.Blocks.Interfaces.RealInput w_ref(unit="rad/s") "Reference speed"
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Units.Angle phi(start=0, fixed=true);
      Units.AngularVelocity w(start=0, fixed=true);
    equation
      phi = flange.phi;
      der(phi) = w;
      der(w) = (w_ref - w)/T;
    end Speed;
  end Sources;

  package Sensors "Sensors for rotational quantities"
    extends Daedalus.Icons.Package;

    model SpeedSensor "Absolute angular velocity of a flange"
      Interfaces.Flange_a flange annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Daedalus.Blocks.Interfaces.RealOutput w(unit="rad/s") annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    equation
      w = der(flange.phi);
      0 = flange.tau;
      annotation (Icon(graphics={Ellipse(extent={{-70,70},{70,-70}}, fillColor={245,245,245}, fillPattern=FillPattern.Solid),
          Text(extent={{-50,30},{50,-30}}, textString="w")}));
    end SpeedSensor;

    model AngleSensor "Absolute angle of a flange"
      Interfaces.Flange_a flange annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Daedalus.Blocks.Interfaces.RealOutput phi(unit="rad") annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    equation
      phi = flange.phi;
      0 = flange.tau;
    end AngleSensor;

    model TorqueSensor "Torque transmitted from flange_a to flange_b"
      extends Interfaces.PartialTwoFlanges;
      Daedalus.Blocks.Interfaces.RealOutput tau(unit="N.m")
        annotation (Placement(transformation(origin={0,-110}, extent={{-10,-10},{10,10}}, rotation=270)));
    equation
      flange_a.phi = flange_b.phi;
      flange_a.tau = tau;
      flange_b.tau = -tau;
    end TorqueSensor;
  end Sensors;
end Rotational;
