within Daedalus;
package Visualizers "3-D shapes for the workbench Animation view: they only draw, and change no result"
  extends Daedalus.Icons.Package;

  record Orientation "Rotation from the world frame to a shape frame (named like MultiBody.Frames.Orientation)"
    Real T[3, 3] "Transformation matrix: v_shape = T*v_world";
    Real w[3] "Angular velocity, shape frame";
  end Orientation;

  model World "Scene settings; name the instance world so the Animation view finds the up direction"
    parameter Real n[3]={0, 0, -1} "Gravity direction: the view puts up opposite to it";
    annotation (Icon(graphics={
        Rectangle(extent={{-100,100},{100,-100}}, lineColor={0,0,0}, fillColor={245,245,245}, fillPattern=FillPattern.Solid),
        Line(points={{-60,-60},{-60,70}}, color={0,0,0}, thickness=0.5),
        Line(points={{-60,-60},{70,-60}}, color={0,0,0}, thickness=0.5),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}));
  end World;

  model Shape "One shape, with the variables of MultiBody's Visualizers.Advanced.Shape, rotated by phi about n"
    parameter String shapeType="box" "box, cylinder, sphere, cone, pipe, spring, or a modelica:// STL/OBJ file";
    parameter Real n[3]={0, 0, 1} "Axis of the rotation phi (unit vector, world frame)";
    input Real phi=0 "Rotation of the shape frame about n";
    input Real r[3]={0, 0, 0} "Origin of the shape frame, world frame";
    input Real r_shape[3]={0, 0, 0} "Start of the shape, shape frame";
    input Real lengthDirection[3]={1, 0, 0} "Length direction, shape frame";
    input Real widthDirection[3]={0, 1, 0} "Width direction, shape frame";
    input Real length=1;
    input Real width=0.1;
    input Real height=0.1;
    input Real extra=0 "cone: top/bottom diameter ratio; pipe: inner/outer; spring: number of windings";
    input Real color[3]={128, 128, 128} "RGB, 0 to 255";
    input Real specularCoefficient=0.7 "0 matte to 1 glossy";
    Orientation R "Shape frame";
  equation
    R.T = {{n[1]*n[1]*(1 - cos(phi)) + cos(phi), n[1]*n[2]*(1 - cos(phi)) + n[3]*sin(phi), n[1]*n[3]*(1 - cos(phi)) - n[2]*sin(phi)},
           {n[2]*n[1]*(1 - cos(phi)) - n[3]*sin(phi), n[2]*n[2]*(1 - cos(phi)) + cos(phi), n[2]*n[3]*(1 - cos(phi)) + n[1]*sin(phi)},
           {n[3]*n[1]*(1 - cos(phi)) + n[2]*sin(phi), n[3]*n[2]*(1 - cos(phi)) - n[1]*sin(phi), n[3]*n[3]*(1 - cos(phi)) + cos(phi)}};
    R.w = zeros(3);
    annotation (Icon(graphics={
        Rectangle(extent={{-80,40},{60,-60}}, lineColor={0,0,0}, fillColor={160,170,190}, fillPattern=FillPattern.Solid),
        Polygon(points={{-80,40},{-50,70},{90,70},{60,40},{-80,40}}, lineColor={0,0,0}, fillColor={200,210,225}, fillPattern=FillPattern.Solid),
        Polygon(points={{60,40},{90,70},{90,-30},{60,-60},{60,40}}, lineColor={0,0,0}, fillColor={120,130,150}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,130},{150,90}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>The workbench Animation view draws every component that has the result variables of
<code>Modelica.Mechanics.MultiBody.Visualizers.Advanced.Shape</code> (<code>r</code>,
<code>R.T</code>, <code>r_shape</code>, <code>lengthDirection</code>, <code>widthDirection</code>,
<code>length</code>, <code>width</code>, <code>height</code>, <code>extra</code>, <code>color</code>,
<code>specularCoefficient</code>). This model has the same variables without needing the
Modelica Standard Library, so 1-D models can be animated: bind <code>r</code>, <code>phi</code>
or the sizes to the model's positions and angles.</p>
</html>"));
  end Shape;

  model Car "Saloon car (STL meshes) driving along x; the wheels turn with the distance"
    parameter Real y0=0 "Lane (lateral position)";
    parameter Real color[3]={30, 90, 170} "Paint";
    parameter Real R_wheel=0.32 "Wheel radius the meshes are made for";
    parameter Real wheelbase=2.7 "Wheelbase the meshes are made for";
    input Real x "Rear-axle position along the road";
  protected
    final parameter Real xw[4]={wheelbase, wheelbase, 0, 0};
    final parameter Real yw[4]={0.78, -0.78, 0.78, -0.78};
    final parameter Real side[4]={1, -1, 1, -1};
    Real phi = x/R_wheel "Wheel angle";
    Shape body(shapeType="modelica://Daedalus/Resources/Shapes/car_body.stl", r={x, y0, 0}, color=color, specularCoefficient=0.9);
    Shape glass(shapeType="modelica://Daedalus/Resources/Shapes/car_glass.stl", r={x, y0, 0}, color={25, 32, 45}, specularCoefficient=1);
    Shape trim(shapeType="modelica://Daedalus/Resources/Shapes/car_trim.stl", r={x, y0, 0}, color={30, 30, 34}, specularCoefficient=0.3);
    Shape lamps(shapeType="modelica://Daedalus/Resources/Shapes/car_lamps.stl", r={x, y0, 0}, color={250, 248, 235}, specularCoefficient=1);
    Shape tail(shapeType="modelica://Daedalus/Resources/Shapes/car_tail.stl", r={x, y0, 0}, color={200, 20, 25}, specularCoefficient=1);
    Shape tyre[4](each shapeType="modelica://Daedalus/Resources/Shapes/car_tyre.stl", each n={0, 1, 0}, each phi=phi, r={{x + xw[i], y0 + yw[i], R_wheel} for i in 1:4},
      lengthDirection={{0, side[i], 0} for i in 1:4}, each widthDirection={1, 0, 0}, each color={28, 28, 31}, each specularCoefficient=0.15);
    Shape rim[4](each shapeType="modelica://Daedalus/Resources/Shapes/car_rim.stl", each n={0, 1, 0}, each phi=phi, r={{x + xw[i], y0 + yw[i], R_wheel} for i in 1:4},
      lengthDirection={{0, side[i], 0} for i in 1:4}, each widthDirection={1, 0, 0}, each color={200, 204, 212}, each specularCoefficient=0.9);
    annotation (Icon(graphics={
        Polygon(points={{-90,-20},{-90,20},{-50,30},{-20,60},{50,60},{80,20},{90,15},{90,-20},{-90,-20}},
          lineColor={30,60,110}, fillColor={30,90,170}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-70,-5},{-30,-45}}, fillColor={40,40,40}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{40,-5},{80,-45}}, fillColor={40,40,40}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,110},{150,70}}, textString="%name", textColor={0,0,255})}));
  end Car;

  model QuarterCar "Quarter car seen in 3-D: road pad, tyre, hub, coil spring, damper and body"
    parameter Real y0=0 "Lateral position";
    parameter Real R_tyre=0.28 "Tyre radius drawn";
    parameter Real color[3]={200, 40, 40} "Body";
    input Real z_road "Road (pad) height";
    input Real z_wheel "Wheel centre height";
    input Real z_body "Body reference height";
    input Real F_act=0 "Actuator force, drawn as an arrow (0: none)";
    parameter Real N_to_m=1/4000 "Arrow length per newton";
  protected
    Shape road(r={0, y0, z_road}, r_shape={0, 0, -0.06}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=0.06, width=0.7, height=0.5, color={70, 75, 85}, specularCoefficient=0.3);
    Shape ram(shapeType="cylinder", r={0, y0, z_road - 0.06}, lengthDirection={0, 0, -1}, widthDirection={1, 0, 0},
      length=z_road + 0.4, width=0.07, height=0.07, color={215, 220, 228}, specularCoefficient=1);
    Shape tyre(shapeType="pipe", r={0, y0, z_wheel}, r_shape={0, 0.11, 0}, lengthDirection={0, -1, 0}, widthDirection={1, 0, 0},
      length=0.22, width=2*R_tyre, height=2*R_tyre, extra=0.62, color={28, 28, 32}, specularCoefficient=0.2);
    Shape rim(shapeType="cylinder", r={0, y0, z_wheel}, r_shape={0, 0.1, 0}, lengthDirection={0, -1, 0}, widthDirection={1, 0, 0},
      length=0.2, width=1.24*R_tyre, height=1.24*R_tyre, color={190, 195, 205}, specularCoefficient=0.9);
    Shape hub(shapeType="cylinder", r={0, y0, z_wheel}, lengthDirection={0, -1, 0}, widthDirection={1, 0, 0},
      length=0.42, width=0.06, height=0.06, color={95, 100, 110});
    Shape seat(r={0, y0 - 0.42, z_wheel}, r_shape={-0.07, 0, 0}, length=0.14, width=0.2, height=0.04, color={95, 100, 110});
    Shape spring(shapeType="spring", r={0, y0 - 0.36, z_wheel + 0.02}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=z_body - z_wheel + 0.03, width=0.09, height=0.016, extra=6, color={230, 180, 0});
    Shape damperTube(shapeType="cylinder", r={0, y0 - 0.48, z_wheel + 0.02}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=0.16, width=0.045, height=0.045, color={40, 40, 45});
    Shape damperRod(shapeType="cylinder", r={0, y0 - 0.48, z_body + 0.05}, lengthDirection={0, 0, -1}, widthDirection={1, 0, 0},
      length=z_body - z_wheel - 0.02, width=0.018, height=0.018, color={215, 220, 228}, specularCoefficient=1);
    Shape body(r={0, y0, z_body + 0.05}, r_shape={0, -0.2, 0}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=0.1, width=0.62, height=0.62, color=color, specularCoefficient=0.85) "Quarter of the body: a floor panel";
    Shape arrow(shapeType="cylinder", r={0, y0 - 0.6, z_wheel + 0.02}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=abs(F_act)*N_to_m, width=0.035, height=0.035, color=if F_act > 0 then {0, 160, 80} else {220, 60, 40});
    annotation (Icon(graphics={
        Rectangle(extent={{-60,90},{60,50}}, lineColor={0,0,0}, fillColor={200,40,40}, fillPattern=FillPattern.Solid),
        Line(points={{-30,50},{-40,40},{-20,30},{-40,20},{-20,10},{-30,0}}, color={0,0,0}),
        Ellipse(extent={{-40,10},{40,-70}}, lineColor={0,0,0}, fillColor={40,40,40}, fillPattern=FillPattern.Solid),
        Line(points={{-80,-80},{80,-80}}, color={0,0,0}),
        Text(extent={{-150,140},{150,100}}, textString="%name", textColor={0,0,255})}));
  end QuarterCar;

  model LandingGear "Main gear in 3-D: wing segment, oleo strut, twin wheels and the runway"
    parameter Real y0=0 "Lateral position";
    parameter Real R_tyre=0.6 "Tyre radius";
    input Real z_axle "Axle height";
    input Real z_top "Strut top (sprung mass) height";
  protected
    Shape runway(r={0, y0, 0}, r_shape={-2, 0, -0.08}, lengthDirection={1, 0, 0}, widthDirection={0, 1, 0},
      length=4, width=1.8, height=0.08, color={75, 78, 84}, specularCoefficient=0.2);
    Shape stripe(r={0, y0, 0}, r_shape={-2, 0, 0}, lengthDirection={1, 0, 0}, widthDirection={0, 1, 0},
      length=4, width=0.12, height=0.004, color={235, 235, 235}, specularCoefficient=0.2);
    Shape tyreL(shapeType="pipe", r={0, y0 + 0.2, z_axle}, lengthDirection={0, 1, 0}, widthDirection={1, 0, 0},
      length=0.32, width=2*R_tyre, height=2*R_tyre, extra=0.55, color={28, 28, 32}, specularCoefficient=0.2);
    Shape tyreR(shapeType="pipe", r={0, y0 - 0.52, z_axle}, lengthDirection={0, 1, 0}, widthDirection={1, 0, 0},
      length=0.32, width=2*R_tyre, height=2*R_tyre, extra=0.55, color={28, 28, 32}, specularCoefficient=0.2);
    Shape hubL(shapeType="cylinder", r={0, y0 + 0.21, z_axle}, lengthDirection={0, 1, 0}, widthDirection={1, 0, 0},
      length=0.3, width=1.1*R_tyre, height=1.1*R_tyre, color={185, 190, 200}, specularCoefficient=0.9);
    Shape hubR(shapeType="cylinder", r={0, y0 - 0.51, z_axle}, lengthDirection={0, 1, 0}, widthDirection={1, 0, 0},
      length=0.3, width=1.1*R_tyre, height=1.1*R_tyre, color={185, 190, 200}, specularCoefficient=0.9);
    Shape axle(shapeType="cylinder", r={0, y0 - 0.25, z_axle}, lengthDirection={0, 1, 0}, widthDirection={1, 0, 0},
      length=0.5, width=0.12, height=0.12, color={95, 100, 110});
    Shape piston(shapeType="cylinder", r={0, y0, z_axle}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=z_top - z_axle - 0.1, width=0.13, height=0.13, color={220, 225, 232}, specularCoefficient=1);
    Shape sleeve(shapeType="cylinder", r={0, y0, z_top + 0.3}, lengthDirection={0, 0, -1}, widthDirection={1, 0, 0},
      length=0.3 + 0.55*(z_top - z_axle), width=0.22, height=0.22, color={150, 155, 165}, specularCoefficient=0.7);
    Shape wing(shapeType="modelica://Daedalus/Resources/Shapes/gear_wing.stl", r={0, y0, z_top + 0.55},
      color={225, 228, 235}, specularCoefficient=0.8) "Wing segment carrying the gear";
    annotation (Icon(graphics={
        Rectangle(extent={{-80,90},{80,50}}, lineColor={0,0,0}, fillColor={230,232,238}, fillPattern=FillPattern.Solid),
        Rectangle(extent={{-10,50},{10,-20}}, lineColor={0,0,0}, fillColor={150,155,165}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-40,-10},{40,-90}}, lineColor={0,0,0}, fillColor={40,40,40}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,140},{150,100}}, textString="%name", textColor={0,0,255})}));
  end LandingGear;

  model Aileron "Wing section, aileron on its hinge, control horn and hydraulic actuator"
    parameter Real span=1.6;
    parameter Real r_horn=0.05 "Horn radius";
    parameter Real scaleHorn=3 "Drawing scale of the horn and actuator (the real ones are small)";
    input Real phi "Aileron deflection about the hinge line (x)";
  protected
    final parameter Real rh=r_horn*scaleHorn;
    Real tip[3] = {span/2 + 0.05, -rh*sin(phi), -rh*cos(phi)} "Horn tip";
    Shape wing(shapeType="modelica://Daedalus/Resources/Shapes/wing.stl", color={215, 220, 228}, specularCoefficient=0.8)
      "Wing box ahead of the hinge (mesh made for span = 1.6)";
    Shape aileron(shapeType="modelica://Daedalus/Resources/Shapes/aileron.stl", n={1, 0, 0}, phi=phi,
      color={200, 40, 40}, specularCoefficient=0.85);
    Shape hinge(shapeType="cylinder", r={-span/2 - 0.04, 0, 0}, length=span + 0.08, width=0.035, height=0.035,
      color={60, 60, 66});
    Shape horn(n={1, 0, 0}, phi=phi, r={span/2 + 0.05, 0, 0}, r_shape={-0.015, 0, 0}, lengthDirection={0, 0, -1},
      widthDirection={1, 0, 0}, length=rh, width=0.03, height=0.03, color={60, 60, 66});
    Shape barrel(shapeType="cylinder", r={span/2 + 0.05, 0.75, -rh}, lengthDirection={0, -1, 0}, widthDirection={1, 0, 0},
      length=0.4, width=0.09, height=0.09, color={30, 90, 170}, specularCoefficient=0.7);
    Shape rod(shapeType="cylinder", r={span/2 + 0.05, 0.35, -rh}, lengthDirection=tip - {span/2 + 0.05, 0.35, -rh},
      widthDirection={1, 0, 0}, length=Daedalus.Visualizers.norm(tip - {span/2 + 0.05, 0.35, -rh}), width=0.035, height=0.035,
      color={215, 220, 228}, specularCoefficient=1);
    annotation (Icon(graphics={
        Polygon(points={{-90,10},{20,10},{20,-10},{-90,-10},{-90,10}}, lineColor={0,0,0}, fillColor={215,220,228}, fillPattern=FillPattern.Solid),
        Polygon(points={{24,8},{90,-20},{24,-8},{24,8}}, lineColor={0,0,0}, fillColor={200,40,40}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,80},{150,40}}, textString="%name", textColor={0,0,255})}));
  end Aileron;

  model RotatingDisc "Disc that turns about x, with two marks so the rotation shows"
    parameter Real r0[3]={0, 0, 0} "Centre";
    parameter Real diameter=0.4;
    parameter Real thickness=0.05;
    parameter Real color[3]={150, 155, 165};
    input Real phi "Angle about x";
  protected
    Shape disc(shapeType="cylinder", r=r0, r_shape={-thickness/2, 0, 0}, length=thickness, width=diameter, height=diameter,
      color=color, specularCoefficient=0.8);
    Shape mark1(n={1, 0, 0}, phi=phi, r=r0, r_shape={-thickness/2 - 0.005, -0.03, 0}, lengthDirection={1, 0, 0},
      widthDirection={0, 1, 0}, length=thickness + 0.01, width=0.06, height=diameter*0.48, color={230, 180, 0});
    Shape mark2(n={1, 0, 0}, phi=phi + 3.141592653589793, r=r0, r_shape={-thickness/2 - 0.005, -0.03, 0}, lengthDirection={1, 0, 0},
      widthDirection={0, 1, 0}, length=thickness + 0.01, width=0.06, height=diameter*0.48, color={40, 40, 45});
  end RotatingDisc;

  model Gauge "Vertical bar whose height shows a value"
    parameter Real r0[3]={0, 0, 0} "Foot of the bar";
    parameter Real scale=1 "Bar height per unit of the value";
    parameter Real color[3]={200, 40, 40};
    parameter Real width=0.08;
    parameter Real full=1 "Value drawn as the full-scale frame";
    input Real value;
  protected
    Shape bar(r=r0, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0}, length=max(1e-4, abs(value)*scale), width=width,
      height=width, color=color, specularCoefficient=0.6);
    Shape frame(shapeType="pipe", r=r0, r_shape={0, 0, 0}, lengthDirection={0, 0, 1}, widthDirection={1, 0, 0},
      length=full*scale, width=1.5*width, height=1.5*width, extra=0.85, color={180, 185, 195}, specularCoefficient=0.3);
  end Gauge;

  function norm "Length of a 3-vector"
    input Real v[3];
    output Real y;
  algorithm
    y := sqrt(v*v);
  end norm;

  annotation (Documentation(info="<html>
<p>Shapes for the 3-D Animation view of the workbench, for models that do not use
<code>Modelica.Mechanics.MultiBody</code>. Each example in <code>Daedalus.Examples</code> places a
<code>World</code> named <code>world</code> and a few of these assemblies bound to its positions,
angles and speeds, the way Dymola animates 1-D libraries.</p>
</html>"));
end Visualizers;
