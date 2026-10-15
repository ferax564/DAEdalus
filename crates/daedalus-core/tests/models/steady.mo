package Steady
  model FirstOrder "T der(x) = k u - x: equilibrium x = k u"
    parameter Real k = 2.5;
    parameter Real T = 0.7;
    input Real u(start = 2);
    output Real y;
    Real x(start = 0, fixed = true);
  equation
    T * der(x) = k * u - x;
    y = x;
  end FirstOrder;

  model Tank "A der(h) = q - c sqrt(h): equilibrium h = (q / c)^2"
    parameter Real A = 2;
    parameter Real c = 0.5;
    parameter Real cEff = 2 * c "computed from c";
    input Real q(start = 1);
    output Real level;
    Real h(start = 1, fixed = true);
  equation
    A * der(h) = q - 0.5 * cEff * sqrt(h);
    level = h;
  end Tank;

  model Pendulum "damped pendulum with a constant torque: sin(phi) = tau / (m g L)"
    parameter Real m = 1;
    parameter Real L = 0.8;
    parameter Real g = 9.81;
    parameter Real d = 0.3;
    input Real tau(start = 2);
    output Real angle;
    output Real x;
    output Real yPos;
    Real phi(start = 0.9, fixed = true);
    Real w(start = 0, fixed = true);
  equation
    der(phi) = w;
    m * L ^ 2 * der(w) = -m * g * L * sin(phi) - d * w + tau;
    angle = phi;
    x = L * sin(phi);
    yPos = -L * cos(phi);
  end Pendulum;

  model Bistable "der(x) = x - x^3: equilibria -1, 0, 1"
    output Real y;
    Real x(start = 0.1, fixed = true);
  equation
    der(x) = x - x ^ 3;
    y = x;
  end Bistable;

  model Loop "a state behind a nonlinear algebraic loop"
    input Real u(start = 1);
    output Real y;
    Real x(start = 0.5, fixed = true);
    Real a;
  equation
    a + 0.3 * sin(a) = x;
    der(x) = u - a ^ 3;
    y = a;
  end Loop;

  model FreeMass "a free mass with friction: the position is not determined"
    parameter Real m = 1;
    parameter Real c = 0.4;
    input Real f(start = 0);
    output Real v;
    Real s(start = 3, fixed = true);
    Real vel(start = 2, fixed = true);
  equation
    der(s) = vel;
    m * der(vel) = f - c * vel;
    v = vel;
  end FreeMass;
end Steady;
