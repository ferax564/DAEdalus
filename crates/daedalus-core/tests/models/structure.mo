package Struct
  model MassSpringDamper "m der(v) = -c v - k x: eigenvalues -c/2m +- i sqrt(k/m - (c/2m)^2)"
    parameter Real m = 2;
    parameter Real c = 0.5;
    parameter Real k = 8;
    Real x(start = 0.1, fixed = true);
    Real v(start = 0, fixed = true);
  equation
    der(x) = v;
    m * der(v) = -c * v - k * x;
  end MassSpringDamper;

  model Stiff "two first-order lags with time constants 1 s and 1 ms"
    Real a(start = 1, fixed = true);
    Real b(start = 1, fixed = true);
  equation
    der(a) = -a;
    der(b) = -1000 * b + a;
  end Stiff;

  model Loop "a nonlinear algebraic loop in (y, z)"
    Real x(start = 1, fixed = true);
    Real y(start = 0.5);
    Real z;
  equation
    der(x) = -y;
    y = z + 0.1 * sin(y) + x;
    z = 0.5 * y - x * z;
  end Loop;

  model Nonlinear "a nonlinear oscillator: the Jacobian depends on the state"
    Real x(start = 0.7, fixed = true);
    Real v(start = 0.2, fixed = true);
    Real e;
  equation
    der(x) = v;
    der(v) = -sin(x) - 0.3 * v * x ^ 2 + e;
    e = 0.1 * x * v;
  end Nonlinear;

  model Gear "two inertias coupled by an ideal gear: one of phi1, phi2 is a state"
    parameter Real J1 = 1;
    parameter Real J2 = 2;
    parameter Real ratio = 3;
    parameter Real k = 5;
    Real phi1(start = 0.2, fixed = true);
    Real w1(start = 0, fixed = true);
    Real phi2;
    Real w2;
    Real tau;
  equation
    der(phi1) = w1;
    der(phi2) = w2;
    J1 * der(w1) = -k * phi1 - ratio * tau;
    J2 * der(w2) = tau;
    phi1 = ratio * phi2;
    annotation(experiment(StopTime = 3));
  end Gear;

  model Pendulum "pendulum in cartesian coordinates (index 3), released at 0.5 rad"
    parameter Real L = 1;
    parameter Real m = 1;
    parameter Real g = 9.81;
    Real x(start = 0.479425538604203, fixed = true);
    Real y(start = -0.877582561890373);
    Real vx(start = 0, fixed = true);
    Real vy(start = 0);
    Real F;
  equation
    der(x) = vx;
    der(y) = vy;
    m * der(vx) = -F * x / L;
    m * der(vy) = -F * y / L - m * g;
    x ^ 2 + y ^ 2 = L ^ 2;
    annotation(experiment(StopTime = 2));
  end Pendulum;
end Struct;
