within ;
package Daedalus "DAEdalus component library: blocks, mechanics, hydraulics and vehicles"
  extends Daedalus.Icons.Library;

  annotation (
    version="0.1.0",
    Documentation(info="<html>
<p>The DAEdalus library is a compact, self-contained component library written for
the DAEdalus simulation environment. It covers signal blocks, 1D translational and
rotational mechanics (including friction clutches, backlash and torque converters),
isothermal oil hydraulics (orifices, volumes, servo valves, cylinders, pumps) and
longitudinal vehicle models (road loads, tyres, electric drives, batteries, drivers
and standard drive cycles).</p>
<p>The <code>Examples</code> package reproduces published application examples
from Modelon and Claytex blog posts with openly documented assumptions.</p>
<p>Licensed under the Apache License 2.0.</p>
</html>"),
    Icon(graphics={
        Rectangle(extent={{-100,-100},{100,100}}, lineColor={30,60,110}, fillColor={235,240,250},
          fillPattern=FillPattern.Solid, radius=25),
        Polygon(points={{-60,-50},{0,60},{60,-50},{-60,-50}}, lineColor={30,60,110},
          fillColor={60,110,190}, fillPattern=FillPattern.Solid),
        Line(points={{-30,-50},{0,6},{30,-50}}, color={235,240,250}, thickness=1)}));
end Daedalus;
