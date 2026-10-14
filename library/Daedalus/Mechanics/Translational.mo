within Daedalus.Mechanics;
package Translational "1D translational mechanics: masses, springs, dampers, contacts"
  extends Daedalus.Icons.Package;

  package Interfaces "Translational flanges and partial models"
    extends Daedalus.Icons.Package;

    connector Flange "Translational flange (absolute position and cut force)"
      Units.Position s "Absolute position of flange";
      flow Units.Force f "Cut force directed into flange";
    end Flange;

    connector Flange_a "Translational flange (filled)"
      extends Flange;
      annotation (Icon(graphics={Rectangle(extent={{-100,-100},{100,100}}, lineColor={0,127,0},
          fillColor={0,127,0}, fillPattern=FillPattern.Solid)}));
    end Flange_a;

    connector Flange_b "Translational flange (open)"
      extends Flange;
      annotation (Icon(graphics={Rectangle(extent={{-100,-100},{100,100}}, lineColor={0,127,0},
          fillColor={255,255,255}, fillPattern=FillPattern.Solid)}));
    end Flange_b;

    partial model PartialTwoFlanges "Two translational flanges"
      Flange_a flange_a "Left flange" annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Flange_b flange_b "Right flange" annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      annotation (Icon(graphics={Text(extent={{-150,90},{150,50}}, textString="%name", textColor={0,0,255})}));
    end PartialTwoFlanges;

    partial model PartialCompliant "Compliant connection of two flanges"
      extends PartialTwoFlanges;
      Units.Distance s_rel(start=0) "Relative distance (flange_b.s - flange_a.s)";
      Units.Velocity v_rel(start=0) "Relative velocity";
      Units.Force f "Force between flanges (positive in compression of flange_b)";
    equation
      s_rel = flange_b.s - flange_a.s;
      v_rel = der(s_rel);
      flange_b.f = f;
      flange_a.f = -f;
    end PartialCompliant;

    partial model PartialElementaryOneFlange "One flange"
      Flange_b flange annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      annotation (Icon(graphics={Text(extent={{-150,140},{150,100}}, textString="%name", textColor={0,0,255})}));
    end PartialElementaryOneFlange;
  end Interfaces;

  package Components "Translational components"
    extends Daedalus.Icons.Package;

    model Fixed "Flange fixed at a position"
      parameter Units.Position s0=0 "Fixed position";
      Interfaces.Flange_b flange annotation (Placement(transformation(extent={{-10,-10},{10,10}})));
    equation
      flange.s = s0;
      annotation (Icon(graphics={Line(points={{-80,-40},{80,-40}}), Line(points={{-80,-40},{-60,-60}}),
          Line(points={{-40,-40},{-20,-60}}), Line(points={{0,-40},{20,-60}}), Line(points={{40,-40},{60,-60}}),
          Line(points={{0,-40},{0,-10}})}));
    end Fixed;

    model Mass "Sliding mass with optional gravity along the flange axis"
      parameter Units.Mass m(min=0, start=1) "Mass";
      parameter Units.Acceleration g=0 "Gravity acceleration along +s (e.g. -9.81 for s pointing up)";
      parameter StateSelect stateSelect=StateSelect.default;
      Interfaces.Flange_a flange_a annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Interfaces.Flange_b flange_b annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Units.Position s(stateSelect=stateSelect) "Absolute position of the centre of mass";
      Units.Velocity v(stateSelect=stateSelect) "Absolute velocity";
      Units.Acceleration a "Absolute acceleration";
    equation
      s = flange_a.s;
      s = flange_b.s;
      v = der(s);
      a = der(v);
      m*a = flange_a.f + flange_b.f + m*g;
      annotation (Icon(graphics={
          Line(points={{-100,0},{-55,0}}, color={0,127,0}), Line(points={{55,0},{100,0}}, color={0,127,0}),
          Rectangle(extent={{-55,40},{55,-40}}, lineColor={0,0,0}, fillColor={160,215,160}, fillPattern=FillPattern.Solid),
          Text(extent={{-150,90},{150,50}}, textString="%name", textColor={0,0,255}),
          Text(extent={{-150,-60},{150,-100}}, textString="m=%m")}));
    end Mass;

    model Spring "Linear translational spring"
      extends Interfaces.PartialCompliant;
      parameter Units.TranslationalSpringConstant c(min=0, start=1) "Spring constant";
      parameter Units.Distance s_rel0=0 "Unstretched length";
    equation
      f = c*(s_rel - s_rel0);
      annotation (Icon(graphics={Line(points={{-100,0},{-60,0},{-45,-30},{-15,30},{15,-30},{45,30},{60,0},{100,0}}, color={0,127,0})}));
    end Spring;

    model Damper "Linear translational damper"
      extends Interfaces.PartialCompliant;
      parameter Units.TranslationalDampingConstant d(min=0, start=0) "Damping constant";
      Units.Power lossPower;
    equation
      f = d*v_rel;
      lossPower = f*v_rel;
      annotation (Icon(graphics={Line(points={{-100,0},{-60,0}}, color={0,127,0}),
          Line(points={{-60,-30},{-60,30}}), Line(points={{-60,-30},{60,-30}}), Line(points={{-60,30},{60,30}}),
          Rectangle(extent={{-60,30},{30,-30}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
          Line(points={{30,0},{100,0}}, color={0,127,0})}));
    end Damper;

    model SpringDamper "Linear translational spring and damper in parallel"
      extends Interfaces.PartialCompliant;
      parameter Units.TranslationalSpringConstant c(min=0, start=1);
      parameter Units.TranslationalDampingConstant d(min=0, start=0);
      parameter Units.Distance s_rel0=0;
      Units.Power lossPower;
    equation
      f = c*(s_rel - s_rel0) + d*v_rel;
      lossPower = d*v_rel*v_rel;
      annotation (Icon(graphics={Line(points={{-80,32},{-58,32},{-43,2},{-13,62},{17,2},{47,62},{62,32},{80,32}}, color={0,127,0}),
          Line(points={{-80,-40},{-20,-40}}), Rectangle(extent={{-20,-20},{40,-60}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
          Line(points={{40,-40},{80,-40}}), Line(points={{-100,0},{-80,0},{-80,32}}), Line(points={{-80,0},{-80,-40}}),
          Line(points={{100,0},{80,0},{80,32}}), Line(points={{80,0},{80,-40}})}));
    end SpringDamper;

    model ElastoGap "Unilateral elastic contact that only pushes (Hunt-Crossley style damping)"
      extends Interfaces.PartialCompliant;
      parameter Units.TranslationalSpringConstant c(min=0, start=1) "Contact stiffness";
      parameter Units.TranslationalDampingConstant d(min=0, start=0) "Contact damping";
      parameter Units.Distance s_rel0=0 "Relative distance at which contact starts";
      parameter Real n(min=1)=1 "Exponent of the spring force (1: linear, 1.5: Hertz)";
      Boolean contact "true while the flanges penetrate";
      Units.Distance penetration "Penetration depth (>= 0)";
    equation
      contact = s_rel < s_rel0;
      penetration = if contact then s_rel0 - s_rel else 0;
      // the damper may not pull the contact surfaces together
      f = if contact then min(0, -c*penetration^n + d*v_rel) else 0;
      annotation (Icon(graphics={Line(points={{-100,0},{-50,0}}, color={0,127,0}),
          Line(points={{-50,-40},{-50,40}}, thickness=1), Line(points={{-10,-40},{-10,40}}, thickness=1),
          Line(points={{-10,0},{100,0}}, color={0,127,0})}));
    end ElastoGap;
  end Components;

  package Sources "Force and position sources"
    extends Daedalus.Icons.Package;

    model Force "Input signal acting as external force on a flange"
      extends Interfaces.PartialElementaryOneFlange;
      Daedalus.Blocks.Interfaces.RealInput f(unit="N") "Driving force"
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    equation
      flange.f = -f;
      annotation (Icon(graphics={Line(points={{-96,0},{-40,0}}, color={0,0,127}),
          Polygon(points={{90,0},{30,20},{30,-20},{90,0}}, fillColor={0,127,0}, fillPattern=FillPattern.Solid),
          Line(points={{-40,0},{30,0}}, color={0,127,0})}));
    end Force;

    model ConstantForce "Constant force"
      extends Interfaces.PartialElementaryOneFlange;
      parameter Units.Force f_constant "Constant force (if negative, the force acts as a load)";
    equation
      flange.f = -f_constant;
    end ConstantForce;

    model Position "Flange follows a reference position through a critically damped second-order filter"
      extends Interfaces.PartialElementaryOneFlange;
      parameter Units.Frequency f_crit=50 "Cut-off frequency of the follower";
      Daedalus.Blocks.Interfaces.RealInput s_ref(unit="m")
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Units.Position s(start=0, fixed=true);
      Units.Velocity v(start=0, fixed=true);
    protected
      parameter Real w=2*3.141592653589793*f_crit;
    equation
      s = flange.s;
      der(s) = v;
      der(v) = w*w*(s_ref - s) - 2*w*v;
    end Position;
  end Sources;

  package Sensors "Sensors for translational quantities"
    extends Daedalus.Icons.Package;

    model PositionSensor
      Interfaces.Flange_a flange annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Daedalus.Blocks.Interfaces.RealOutput s(unit="m") annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    equation
      s = flange.s;
      0 = flange.f;
    end PositionSensor;

    model SpeedSensor
      Interfaces.Flange_a flange annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Daedalus.Blocks.Interfaces.RealOutput v(unit="m/s") annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    equation
      v = der(flange.s);
      0 = flange.f;
    end SpeedSensor;

    model ForceSensor "Force transmitted from flange_a to flange_b"
      extends Interfaces.PartialTwoFlanges;
      Daedalus.Blocks.Interfaces.RealOutput f(unit="N")
        annotation (Placement(transformation(origin={0,-110}, extent={{-10,-10},{10,10}}, rotation=270)));
    equation
      flange_a.s = flange_b.s;
      flange_a.f = f;
      flange_b.f = -f;
    end ForceSensor;
  end Sensors;
end Translational;
