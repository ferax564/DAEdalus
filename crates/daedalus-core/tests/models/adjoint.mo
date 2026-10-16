package Adjoint "models for adjoint gradients and designs of experiments"
  model Chain "a chain of nonlinear first-order stages, a rate per stage"
    parameter Real k1 = 1.0;
    parameter Real k2 = 1.3;
    parameter Real k3 = 0.8;
    parameter Real k4 = 1.7;
    parameter Real k5 = 0.6;
    parameter Real k6 = 1.1;
    parameter Real g = 0.9 "input gain";
    parameter Real w = 2.0 "input frequency";
    parameter Real e = 1.5 "output exponent";
    parameter Real kk = 2 * k1 "computed from k1";
    Real u = g * (1 + 0.5 * sin(w * time));
    Real x1(start = 0.2, fixed = true);
    Real x2(start = 0, fixed = true);
    Real x3(start = 0, fixed = true);
    Real x4(start = 0, fixed = true);
    Real x5(start = 0, fixed = true);
    Real x6(start = 0, fixed = true);
    Real y = (1 + x6 ^ 2) ^ e "a nonlinear output";
    Real z = sqrt(1 + x3 ^ 2) * exp(-0.1 * x4) / (1 + abs(x5));
  equation
    der(x1) = u - kk / 2 * x1;
    der(x2) = k1 * tanh(x1) - k2 * x2 / (1 + x2 ^ 2);
    der(x3) = k2 * x2 - k3 * x3;
    der(x4) = k3 * x3 - k4 * x4 + 0.1 * atan(x2);
    der(x5) = k4 * x4 - k5 * x5 ^ 3 - k5 * x5;
    der(x6) = k5 * max(x5, 0) - k6 * x6;
    annotation(experiment(StopTime = 4));
  end Chain;

  model Stiff "a stiff linear system with a slow and a fast mode"
    parameter Real a = 1;
    parameter Real b = 500;
    parameter Real c = 2;
    Real x(start = 1, fixed = true);
    Real v(start = 0, fixed = true);
  equation
    der(x) = -a * x + c * v;
    der(v) = -b * (v - sin(x));
    annotation(experiment(StopTime = 2));
  end Stiff;

  model Quad "y = 1 + 2 a - b + 0.5 a c + 3 b^2 (in coded units over [0, 2])"
    parameter Real a = 1;
    parameter Real b = 1;
    parameter Real c = 1;
    Real y;
    Real y2;
  equation
    y = 1 + 2 * (a - 1) - (b - 1) + 0.5 * (a - 1) * (c - 1) + 3 * (b - 1) ^ 2 + 0 * time;
    y2 = (a - 1) ^ 2 + (b - 1.5) ^ 2 + 0 * time;
  end Quad;
end Adjoint;
