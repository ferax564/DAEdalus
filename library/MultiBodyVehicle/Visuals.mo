within MultiBodyVehicle;
package Visuals "Animation-only parts: they draw, but add no mass, force or state"
  extends Modelica.Icons.Package;
  import MB = Modelica.Mechanics.MultiBody;
  import SI = Modelica.Units.SI;

  model Wheel "Treaded tyre, spoked rim, brake disc and caliper, drawn at a wheel centre; the tyre and rim can spin"
    parameter Real side=1 "+1 left, -1 right: the rim face points outboard";
    parameter Boolean drawBrake=true "Draw the disc and caliper";
    input SI.Position r[3] "Wheel centre, world frame";
    input MB.Frames.Orientation R "Orientation of the non-spinning wheel frame (spin axis = y)";
    input SI.Angle phi=0 "Spin angle about y (positive rolling forward)";
  protected
    Real e_spin[3]={cos(phi), 0, -sin(phi)} "Spinning reference axis, normal to the spin axis";
    MB.Visualizers.Advanced.Shape tyre(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/tyre.stl", r=r, R=R, lengthDirection={0, side, 0},
      widthDirection=e_spin, length=1, width=1, height=1, extra=0, color={34,34,37}, specularCoefficient=0.1);
    MB.Visualizers.Advanced.Shape rim(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/rim.stl", r=r, R=R, lengthDirection={0, side, 0},
      widthDirection=e_spin, length=1, width=1, height=1, extra=0, color={196,200,210}, specularCoefficient=0.9);
    MB.Visualizers.Advanced.Shape disc(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/disc.stl", r=r, R=R, lengthDirection={0, side, 0},
      widthDirection=e_spin, length=1, width=1, height=1, extra=0, color={125,122,120}, specularCoefficient=0.7) if drawBrake;
    MB.Visualizers.Advanced.Shape caliper(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/caliper.stl", r=r, R=R, lengthDirection={0, side, 0},
      widthDirection={-1, 0, 0}, length=1, width=1, height=1, extra=0, color={205,30,35}, specularCoefficient=0.5) if drawBrake;
    annotation (Icon(graphics={
        Ellipse(extent={{-60,90},{60,-90}}, lineColor={0,0,0}, fillColor={40,40,40}, fillPattern=FillPattern.Solid),
        Ellipse(extent={{-34,52},{34,-52}}, lineColor={0,0,0}, fillColor={200,200,210}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,140},{150,100}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>Meshes from <code>Resources/Shapes</code> (made by <code>generate.py</code> there): a 620 x 225 mm
tyre with a block tread, a five-spoke rim, a brake disc and a caliper. The spin angle turns the tyre,
rim and disc about the wheel's y axis, so the tread shows the wheel rolling.</p>
</html>"));
  end Wheel;

  model AArm "Wishbone drawn as two tubes from front and rear chassis bushings to the ball joint"
    extends MB.Interfaces.PartialVisualizer;
    parameter Boolean animation=true;
    parameter SI.Position r_ball[3] "Ball joint, in frame_a (frame_a is on the pivot axis)";
    parameter SI.Distance spread=0.16 "Bushings sit at +-spread along the pivot axis (x of frame_a)";
    parameter SI.Diameter diameter=0.028 "Tube diameter";
    parameter MB.Types.Color color={160,165,175};
    parameter MB.Types.Color bushingColor={40,40,44};
  protected
    final parameter Real legF[3]=r_ball - {spread, 0, 0};
    final parameter Real legR[3]=r_ball + {spread, 0, 0};
    MB.Visualizers.Advanced.Shape front(shapeType="cylinder", r=frame_a.r_0, R=frame_a.R, r_shape={spread, 0, 0},
      lengthDirection=legF, widthDirection={1,0,0}, length=Modelica.Math.Vectors.length(legF), width=diameter, height=diameter,
      color=color, specularCoefficient=0.8) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape rear(shapeType="cylinder", r=frame_a.r_0, R=frame_a.R, r_shape={-spread, 0, 0},
      lengthDirection=legR, widthDirection={1,0,0}, length=Modelica.Math.Vectors.length(legR), width=diameter, height=diameter,
      color=color, specularCoefficient=0.8) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape bushF(shapeType="cylinder", r=frame_a.r_0, R=frame_a.R, r_shape={spread - 0.03, 0, 0},
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=0.06, width=0.05, height=0.05,
      color=bushingColor, specularCoefficient=0.3) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape bushR(shapeType="cylinder", r=frame_a.r_0, R=frame_a.R, r_shape={-spread - 0.03, 0, 0},
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=0.06, width=0.05, height=0.05,
      color=bushingColor, specularCoefficient=0.3) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape ball(shapeType="sphere", r=frame_a.r_0, R=frame_a.R, r_shape=r_ball - {0.025, 0, 0},
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=0.05, width=0.05, height=0.05,
      color={70,70,75}, specularCoefficient=0.9) if world.enableAnimation and animation;
  equation
    frame_a.f = zeros(3);
    frame_a.t = zeros(3);
    annotation (Icon(graphics={
        Polygon(points={{-80,40},{80,0},{-80,-40},{-60,0},{-80,40}}, lineColor={0,0,0}, fillColor={160,165,175}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,100},{150,60}}, textString="%name", textColor={0,0,255})}));
  end AArm;

  model Bar "Box or tube between two points of one frame"
    extends MB.Interfaces.PartialVisualizer;
    parameter Boolean animation=true;
    parameter String shapeType="box";
    parameter SI.Position r_a[3]={0,0,0} "Start, in frame_a";
    parameter SI.Position r_b[3] "End, in frame_a";
    parameter SI.Distance width=0.05;
    parameter SI.Distance height=width;
    parameter Real widthDirection[3]={1,0,0};
    parameter MB.Types.Color color={95,100,110};
    parameter Real specularCoefficient=0.7;
  protected
    MB.Visualizers.Advanced.Shape shape(shapeType=shapeType, r=frame_a.r_0, R=frame_a.R, r_shape=r_a,
      lengthDirection=r_b - r_a, widthDirection=widthDirection, length=Modelica.Math.Vectors.length(r_b - r_a),
      width=width, height=height, color=color, specularCoefficient=specularCoefficient) if world.enableAnimation and animation;
  equation
    frame_a.f = zeros(3);
    frame_a.t = zeros(3);
    annotation (Icon(graphics={
        Rectangle(extent={{-80,15},{80,-15}}, lineColor={0,0,0}, fillColor={95,100,110}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,70},{150,30}}, textString="%name", textColor={0,0,255})}));
  end Bar;

  model BodyShell "Open-wheel race-car body: monocoque, sidepods, engine cover, wings and driver"
    extends MB.Interfaces.PartialVisualizer;
    parameter Boolean animation=true;
    parameter MB.Types.Color color={200,25,35} "Paint";
    parameter MB.Types.Color accentColor={235,235,240} "Livery accent (nose tip, endplates, details)";
  protected
    MB.Visualizers.Advanced.Shape main(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/body_main.stl", r=frame_a.r_0, R=frame_a.R,
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=1, width=1, height=1, extra=0,
      color=color, specularCoefficient=0.85) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape dark(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/body_dark.stl", r=frame_a.r_0, R=frame_a.R,
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=1, width=1, height=1, extra=0,
      color={28,28,32}, specularCoefficient=0.4) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape accent(shapeType="modelica://MultiBodyVehicle/Resources/Shapes/body_accent.stl", r=frame_a.r_0, R=frame_a.R,
      lengthDirection={1,0,0}, widthDirection={0,1,0}, length=1, width=1, height=1, extra=0,
      color=accentColor, specularCoefficient=0.7) if world.enableAnimation and animation;
    MB.Visualizers.Advanced.Shape helmet(shapeType="sphere", r=frame_a.r_0, R=frame_a.R, r_shape={-0.6,0,0.22},
      lengthDirection={-1,0,0}, widthDirection={0,1,0}, length=0.25, width=0.24, height=0.25,
      color={250,200,0}, specularCoefficient=0.9) if world.enableAnimation and animation;
  equation
    frame_a.f = zeros(3);
    frame_a.t = zeros(3);
    annotation (Icon(graphics={
        Polygon(points={{-90,-10},{-60,10},{60,10},{90,0},{60,-20},{-90,-20},{-90,-10}}, lineColor={0,0,0},
          fillColor={200,25,35}, fillPattern=FillPattern.Solid),
        Text(extent={{-150,70},{150,30}}, textString="%name", textColor={0,0,255})}),
      Documentation(info="<html>
<p>Meshes <code>body_main.stl</code> and <code>body_dark.stl</code> in the Vehicle body frame (front axle
centre at wheel-centre height, x forward, z up).</p>
</html>"));
  end BodyShell;
end Visuals;
