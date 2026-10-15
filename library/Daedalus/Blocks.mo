within Daedalus;
package Blocks "Signal blocks: sources, math, continuous and nonlinear blocks"
  extends Daedalus.Icons.Package;

  package Interfaces "Signal connectors and partial blocks"
    extends Daedalus.Icons.Package;

    connector RealInput = input Real "Real input signal" annotation (Icon(graphics={
          Polygon(points={{-100,100},{100,0},{-100,-100},{-100,100}}, lineColor={0,0,127},
            fillColor={0,0,127}, fillPattern=FillPattern.Solid)}),
        Diagram(graphics={Polygon(points={{0,50},{100,0},{0,-50},{0,50}}, lineColor={0,0,127},
            fillColor={0,0,127}, fillPattern=FillPattern.Solid)}));

    connector RealOutput = output Real "Real output signal" annotation (Icon(graphics={
          Polygon(points={{-100,100},{100,0},{-100,-100},{-100,100}}, lineColor={0,0,127},
            fillColor={255,255,255}, fillPattern=FillPattern.Solid)}));

    connector BooleanInput = input Boolean "Boolean input signal" annotation (Icon(graphics={
          Polygon(points={{-100,100},{100,0},{-100,-100},{-100,100}}, lineColor={255,0,255},
            fillColor={255,0,255}, fillPattern=FillPattern.Solid)}));

    connector BooleanOutput = output Boolean "Boolean output signal" annotation (Icon(graphics={
          Polygon(points={{-100,100},{100,0},{-100,-100},{-100,100}}, lineColor={255,0,255},
            fillColor={255,255,255}, fillPattern=FillPattern.Solid)}));

    partial block SO "Single output"
      extends Daedalus.Icons.Block;
      RealOutput y "Output signal" annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    end SO;

    partial block SISO "Single input, single output"
      extends SO;
      RealInput u "Input signal" annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
    end SISO;

    partial block SI2SO "Two inputs, single output"
      extends SO;
      RealInput u1 annotation (Placement(transformation(extent={{-140,40},{-100,80}})));
      RealInput u2 annotation (Placement(transformation(extent={{-140,-80},{-100,-40}})));
    end SI2SO;

    partial block SignalSource "Source with offset and start time"
      extends SO;
      parameter Real offset=0 "Offset of output signal";
      parameter Units.Time startTime=0 "Output = offset for time < startTime";
    end SignalSource;
  end Interfaces;

  package Sources "Signal sources"
    extends Daedalus.Icons.Package;

    block Constant "Constant output"
      extends Interfaces.SO;
      parameter Real k=1 "Constant output value";
    equation
      y = k;
      annotation (Icon(graphics={Line(points={{-80,0},{80,0}}, color={0,0,127}),
          Text(extent={{-100,-20},{100,-60}}, textString="k=%k")}));
    end Constant;

    block Step "Step signal"
      extends Interfaces.SignalSource;
      parameter Real height=1 "Height of step";
    equation
      y = offset + (if time < startTime then 0 else height);
      annotation (Icon(graphics={Line(points={{-80,-60},{0,-60},{0,60},{80,60}}, color={0,0,127})}));
    end Step;

    block Ramp "Ramp signal"
      extends Interfaces.SignalSource;
      parameter Real height=1 "Height of ramp";
      parameter Units.Time duration(min=0)=2 "Duration of ramp";
    equation
      y = offset + (if time < startTime then 0 else if time < startTime + duration then
        (time - startTime)*height/duration else height);
      annotation (Icon(graphics={Line(points={{-80,-60},{-40,-60},{40,60},{80,60}}, color={0,0,127})}));
    end Ramp;

    block Sine "Sine signal"
      extends Interfaces.SignalSource;
      parameter Real amplitude=1 "Amplitude of sine wave";
      parameter Units.Frequency f=1 "Frequency of sine wave";
      parameter Units.Angle phase=0 "Phase of sine wave";
      constant Real pi=3.141592653589793;
    equation
      y = offset + (if time < startTime then 0 else amplitude*sin(2*pi*f*(time - startTime) + phase));
      annotation (Icon(graphics={Line(points={{-80,0},{-60,50},{-40,60},{-20,50},{0,0},{20,-50},{40,-60},
              {60,-50},{80,0}}, color={0,0,127}, smooth=Smooth.Bezier)}));
    end Sine;

    block Pulse "Rectangular pulse train"
      extends Interfaces.SignalSource;
      parameter Real amplitude=1 "Amplitude of pulse";
      parameter Real width(min=0, max=100)=50 "Width of pulse in % of period";
      parameter Units.Time period(min=1e-9)=1 "Time for one period";
    protected
      discrete Units.Time T0(start=startTime, fixed=true) "Start time of current period";
    equation
      when sample(startTime, period) then
        T0 = time;
      end when;
      y = offset + (if time < startTime or time >= T0 + width/100*period then 0 else amplitude);
      annotation (Icon(graphics={Line(points={{-80,-60},{-60,-60},{-60,60},{-20,60},{-20,-60},{20,-60},
              {20,60},{60,60},{60,-60},{80,-60}}, color={0,0,127})}));
    end Pulse;

    block TimeTable "Piecewise linear function of time from a table [time, y]"
      extends Interfaces.SO;
      parameter Real table[:, 2]={{0,0},{1,1}} "Table [time, value]";
      parameter Real timeScale=1 "Factor applied to the table time column";
      parameter Real offset=0;
      parameter Real yScale=1 "Factor applied to the table value column";
      parameter Units.Time shiftTime=0;
    equation
      y = offset + yScale*Functions.interpolate(table, (time - shiftTime)/timeScale, 2);
      annotation (Icon(graphics={Line(points={{-80,-60},{-40,20},{0,0},{40,60},{80,-20}}, color={0,0,127}),
          Line(points={{-80,80},{-80,-80},{80,-80}}, color={160,160,164})}));
    end TimeTable;
  end Sources;

  package Math "Mathematical functions as blocks"
    extends Daedalus.Icons.Package;

    block Gain "Output = k * input"
      parameter Real k=1 "Gain";
      Interfaces.RealInput u annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Interfaces.RealOutput y annotation (Placement(transformation(extent={{100,-10},{120,10}})));
    equation
      y = k*u;
      annotation (Icon(graphics={Polygon(points={{-100,-100},{-100,100},{100,0},{-100,-100}},
          lineColor={0,0,127}, fillColor={255,255,255}, fillPattern=FillPattern.Solid),
          Text(extent={{-90,30},{40,-30}}, textString="%k"),
          Text(extent={{-150,140},{150,100}}, textString="%name", textColor={0,0,255})}));
    end Gain;

    block Add "Output = k1*u1 + k2*u2"
      extends Interfaces.SI2SO;
      parameter Real k1=1;
      parameter Real k2=1;
    equation
      y = k1*u1 + k2*u2;
      annotation (Icon(graphics={Text(extent={{-40,40},{40,-40}}, textString="+")}));
    end Add;

    block Product "Output = u1 * u2"
      extends Interfaces.SI2SO;
    equation
      y = u1*u2;
      annotation (Icon(graphics={Text(extent={{-40,40},{40,-40}}, textString="×")}));
    end Product;

    block Feedback "Output = u1 - u2"
      Interfaces.RealInput u1 annotation (Placement(transformation(extent={{-100,-20},{-60,20}})));
      Interfaces.RealInput u2 annotation (Placement(transformation(extent={{-20,-100},{20,-60}}, rotation=90)));
      Interfaces.RealOutput y annotation (Placement(transformation(extent={{80,-10},{100,10}})));
    equation
      y = u1 - u2;
      annotation (Icon(graphics={Ellipse(extent={{-20,20},{20,-20}}, lineColor={0,0,127},
          fillColor={235,235,235}, fillPattern=FillPattern.Solid),
          Line(points={{-60,0},{-20,0}}, color={0,0,127}), Line(points={{20,0},{80,0}}, color={0,0,127}),
          Line(points={{0,-20},{0,-60}}, color={0,0,127}), Text(extent={{-100,-20},{-20,-60}}, textString="-")}));
    end Feedback;

    block Abs "Output = |input|"
      extends Interfaces.SISO;
    equation
      y = abs(u);
    end Abs;

    block Division "Output = u1 / u2"
      extends Interfaces.SI2SO;
    equation
      y = u1/u2;
    end Division;
  end Math;

  package Continuous "Continuous-time linear blocks"
    extends Daedalus.Icons.Package;

    block Integrator "Output = k * integral of input"
      extends Interfaces.SISO(y(start=y_start));
      parameter Real k=1 "Integrator gain";
      parameter Real y_start=0 "Initial value of output";
      parameter Boolean fixedStart=true "true: y(0) = y_start";
    initial equation
      if fixedStart then
        y = y_start;
      end if;
    equation
      der(y) = k*u;
      annotation (Icon(graphics={Text(extent={{-80,40},{80,-40}}, textString="k/s")}));
    end Integrator;

    block FirstOrder "First order lag: T*der(y) + y = k*u"
      extends Interfaces.SISO(y(start=y_start));
      parameter Real k=1 "Gain";
      parameter Units.Time T(min=1e-12)=1 "Time constant";
      parameter Real y_start=0;
      parameter Boolean steadyStart=false "true: der(y) = 0 at start, else y = y_start";
    initial equation
      if steadyStart then
        der(y) = 0;
      else
        y = y_start;
      end if;
    equation
      der(y) = (k*u - y)/T;
      annotation (Icon(graphics={Text(extent={{-90,40},{90,-40}}, textString="k/(Ts+1)")}));
    end FirstOrder;

    block SecondOrder "Second order: y = k*w^2/(s^2 + 2*D*w*s + w^2) * u"
      extends Interfaces.SISO;
      parameter Real k=1 "Gain";
      parameter Units.AngularVelocity w=1 "Angular frequency";
      parameter Real D=1 "Damping";
      Real yd(start=0) "Derivative of y";
    initial equation
      y = 0;
      yd = 0;
    equation
      der(y) = yd;
      der(yd) = w*(w*(k*u - y) - 2*D*yd);
    end SecondOrder;

    block PI "Proportional-integral controller with anti-windup output limits"
      extends Interfaces.SISO;
      parameter Real k=1 "Proportional gain";
      parameter Units.Time Ti(min=1e-12)=1 "Integral time constant";
      parameter Real yMax=1e30 "Upper output limit";
      parameter Real yMin=-yMax "Lower output limit";
      parameter Real x_start=0 "Initial integrator state";
      Real x(start=x_start, fixed=true) "Integrator state";
      Real yUnlimited;
    equation
      yUnlimited = k*(u + x/Ti);
      y = min(yMax, max(yMin, yUnlimited));
      // conditional integration (anti-windup)
      der(x) = if (yUnlimited > yMax and u > 0) or (yUnlimited < yMin and u < 0) then 0 else u;
      annotation (Icon(graphics={Text(extent={{-80,40},{80,-40}}, textString="PI")}));
    end PI;

    block PID "PID controller with filtered derivative and output limits"
      extends Interfaces.SISO;
      parameter Real k=1 "Gain";
      parameter Units.Time Ti(min=1e-12)=1 "Integral time";
      parameter Units.Time Td(min=0)=0.1 "Derivative time";
      parameter Real Nd(min=1e-12)=10 "Derivative filter: Td/Nd";
      parameter Real yMax=1e30;
      parameter Real yMin=-yMax;
      Real xi(start=0, fixed=true);
      Real xd(start=0, fixed=true);
      Real yUnlimited;
    equation
      der(xd) = (u - xd)*Nd/max(Td, 1e-12);
      yUnlimited = k*(u + xi/Ti + Td*Nd/max(Td, 1e-12)*(u - xd)*(if Td > 0 then 1 else 0));
      y = min(yMax, max(yMin, yUnlimited));
      der(xi) = if (yUnlimited > yMax and u > 0) or (yUnlimited < yMin and u < 0) then 0 else u;
      annotation (Icon(graphics={Text(extent={{-80,40},{80,-40}}, textString="PID")}));
    end PID;
  end Continuous;

  package Nonlinear "Nonlinear blocks"
    extends Daedalus.Icons.Package;

    block Limiter "Limit the range of a signal"
      extends Interfaces.SISO;
      parameter Real uMax=1 "Upper limit";
      parameter Real uMin=-uMax "Lower limit";
    equation
      y = if u > uMax then uMax else if u < uMin then uMin else u;
      annotation (Icon(graphics={Line(points={{-80,-60},{-40,-60},{40,60},{80,60}}, color={0,0,127})}));
    end Limiter;

    block DeadZone "Zero output for inputs inside [uMin, uMax]"
      extends Interfaces.SISO;
      parameter Real uMax=1;
      parameter Real uMin=-uMax;
    equation
      y = if u > uMax then u - uMax else if u < uMin then u - uMin else 0;
    end DeadZone;
  end Nonlinear;

  package Tables "Table lookup blocks"
    extends Daedalus.Icons.Package;

    block Table1D "y = f(u), linear interpolation in table [u, y]"
      extends Interfaces.SISO;
      parameter Real table[:, 2]={{0,0},{1,1}} "Table [u, y] with increasing u";
      parameter Boolean extrapolate=false "true: linear extrapolation, false: hold end values";
    equation
      y = if extrapolate then Functions.interpolateExtrapolate(table, u, 2) else Functions.interpolate(table, u, 2);
      annotation (Icon(graphics={Line(points={{-60,60},{-60,-60},{60,-60},{60,60},{-60,60}}, color={0,0,0}),
          Line(points={{-60,20},{60,20}}), Line(points={{-60,-20},{60,-20}}), Line(points={{0,60},{0,-60}})}));
    end Table1D;
  end Tables;

  package Functions "Built-in table functions"
    extends Daedalus.Icons.Package;

    function interpolate "Linear interpolation in a table, holding end values"
      input Real table[:, :] "Table with increasing first column";
      input Real x "Abscissa";
      input Integer column=2 "Column of the ordinate";
      output Real y;
    protected
      Integer n = size(table, 1);
      Integer i;
    algorithm
      if n == 1 or x <= table[1, 1] then
        y := table[1, column];
      elseif x >= table[n, 1] then
        y := table[n, column];
      else
        i := 1;
        while i < n - 1 and x > table[i + 1, 1] loop
          i := i + 1;
        end while;
        y := if table[i + 1, 1] > table[i, 1] then table[i, column] + (table[i + 1, column] - table[i, column])
          *(x - table[i, 1])/(table[i + 1, 1] - table[i, 1]) else table[i + 1, column];
      end if;
      annotation (__Daedalus_builtin="interpolate");
    end interpolate;

    function interpolateExtrapolate "Linear interpolation in a table with linear extrapolation"
      input Real table[:, :] "Table with increasing first column";
      input Real x "Abscissa";
      input Integer column=2 "Column of the ordinate";
      output Real y;
    protected
      Integer n = size(table, 1);
      Integer i;
    algorithm
      if n == 1 then
        y := table[1, column];
      else
        i := 1;
        while i < n - 1 and x > table[i + 1, 1] loop
          i := i + 1;
        end while;
        y := if table[i + 1, 1] > table[i, 1] then table[i, column] + (table[i + 1, column] - table[i, column])
          *(x - table[i, 1])/(table[i + 1, 1] - table[i, 1]) else table[i + 1, column];
      end if;
      annotation (__Daedalus_builtin="interpolateExtrapolate");
    end interpolateExtrapolate;
  end Functions;
end Blocks;
