package Analysis "models for sensitivity analysis, uncertainty quantification and calibration"
  model Decay "x' = -k x, x(0) = x0"
    parameter Real k = 0.7;
    parameter Real x0 = 2;
    parameter Real k2 = 2 * k "computed from k: not a free parameter";
    Real x(start = x0, fixed = true);
    output Real y = 3 * x "an output computed from the state";
  equation
    der(x) = -k * x;
    annotation(experiment(StopTime = 3));
  end Decay;

  model MassSpringDamper "m x'' = -c x' - k x, x(0) = x0, v(0) = 0"
    parameter Real m = 1.5;
    parameter Real c = 0.4;
    parameter Real k = 6;
    parameter Real x0 = 0.2;
    Real x(start = x0, fixed = true);
    Real v(start = 0, fixed = true);
    Real energy = 0.5 * k * x ^ 2 + 0.5 * m * v ^ 2;
  equation
    der(x) = v;
    m * der(v) = -c * v - k * x;
    annotation(experiment(StopTime = 4));
  end MassSpringDamper;

  model Loop "a nonlinear algebraic loop between state and parameter"
    parameter Real p = 0.8;
    parameter Real q = 0.3;
    Real x(start = 1, fixed = true);
    Real a;
    Real b;
  equation
    der(x) = -p * a;
    a + 0.5 * sin(b) = x + q;
    b = a - 0.1 * b ^ 3;
    annotation(experiment(StopTime = 2));
  end Loop;

  model Bounce "a state event: the slope changes when x crosses 1"
    parameter Real k = 1;
    Real x(start = 0, fixed = true);
  equation
    der(x) = if x < 1 then k else 0.5 * k;
    annotation(experiment(StopTime = 2));
  end Bounce;

  model Linear "x' = -a x + b, x(0) = 0: x(t) = b / a (1 - exp(-a t))"
    parameter Real a = 1;
    parameter Real b = 1;
    Real x(start = 0, fixed = true);
  equation
    der(x) = -a * x + b;
    annotation(experiment(StopTime = 2));
  end Linear;

  model Ishigami "the Ishigami test function of three parameters"
    parameter Real x1 = 0;
    parameter Real x2 = 0;
    parameter Real x3 = 0;
    Real y;
  equation
    y = sin(x1) + 7 * sin(x2) ^ 2 + 0.1 * x3 ^ 4 * sin(x1) + 0 * time;
  end Ishigami;

  model Affine "y = 3 p1 - 2 p2 + 0.5 p3"
    parameter Real p1 = 0;
    parameter Real p2 = 0;
    parameter Real p3 = 0;
    Real y;
  equation
    y = 3 * p1 - 2 * p2 + 0.5 * p3 + 0 * time;
  end Affine;
end Analysis;
