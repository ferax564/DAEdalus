within;
package Modelica "A stand-in for the Modelica Standard Library (tests only)"
  extends Modelica.Icons.Package;

  package Icons
    partial package Package
    end Package;
  end Icons;

  package Blocks
    package Examples
      model Lag "first-order lag of a step"
        parameter Real T = 0.2;
        Real y(start = 0, fixed = true);
        Real u = if time > 0.5 then 1 else 0;
      equation
        T * der(y) = u - y;
        annotation(experiment(StopTime = 2));
      end Lag;
    end Examples;
  end Blocks;
  annotation(version = "4.1.0", uses(ModelicaServices(version = "4.1.0")));
end Modelica;
