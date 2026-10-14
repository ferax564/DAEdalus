within Daedalus;
package Icons "Icon base classes"
  partial package Library "Icon for libraries"
    annotation (Icon(graphics={
        Rectangle(extent={{-100,-100},{100,100}}, lineColor={30,60,110}, fillColor={235,240,250},
          fillPattern=FillPattern.Solid, radius=25)}));
  end Library;

  partial package Package "Icon for standard packages"
    annotation (Icon(graphics={
        Rectangle(extent={{-100,-100},{100,100}}, lineColor={120,130,150}, fillColor={248,248,252},
          fillPattern=FillPattern.Solid, radius=25),
        Rectangle(extent={{-100,-100},{100,100}}, lineColor={120,130,150}, radius=25)}));
  end Package;

  partial package ExamplesPackage "Icon for packages containing runnable examples"
    annotation (Icon(graphics={
        Rectangle(extent={{-100,-100},{100,100}}, lineColor={120,130,150}, fillColor={248,248,252},
          fillPattern=FillPattern.Solid, radius=25),
        Polygon(points={{-36,60},{64,0},{-36,-60},{-36,60}}, lineColor={0,0,0},
          fillColor={60,170,90}, fillPattern=FillPattern.Solid)}));
  end ExamplesPackage;

  partial model Example "Icon for runnable examples"
    annotation (Icon(graphics={
        Ellipse(extent={{-100,-100},{100,100}}, lineColor={60,170,90}, fillColor={255,255,255},
          fillPattern=FillPattern.Solid),
        Polygon(points={{-36,60},{64,0},{-36,-60},{-36,60}}, lineColor={60,170,90},
          fillColor={60,170,90}, fillPattern=FillPattern.Solid)}));
  end Example;

  partial block Block "Icon for signal blocks"
    annotation (Icon(graphics={
        Rectangle(extent={{-100,-100},{100,100}}, lineColor={0,0,127}, fillColor={255,255,255},
          fillPattern=FillPattern.Solid, radius=8),
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}));
  end Block;

  partial model Hydraulic "Icon for hydraulic components"
    annotation (Icon(graphics={
        Text(extent={{-150,150},{150,110}}, textString="%name", textColor={0,0,255})}));
  end Hydraulic;
end Icons;
