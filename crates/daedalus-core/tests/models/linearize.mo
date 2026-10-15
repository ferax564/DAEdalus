package Lin
  model MassSpringDamper "m der(v) = f - c v - k x, position output"
    parameter Real m = 2;
    parameter Real c = 0.5;
    parameter Real k = 8;
    input Real f;
    output Real pos;
    Real x(start = 0.1, fixed = true);
    Real v(start = 0, fixed = true);
  equation
    der(x) = v;
    m * der(v) = f - c * v - k * x;
    pos = x;
  end MassSpringDamper;

  model AlgebraicLoop "a nonlinear algebraic loop in (a, b) between state and output"
    input Real u(start = 0.3);
    output Real y;
    Real x(start = 1, fixed = true);
    Real a;
    Real b;
  equation
    der(x) = -a;
    a + 0.5 * sin(b) = x + u;
    b = a - 0.1 * b ^ 3;
    y = b;
  end AlgebraicLoop;

  model RigidMasses "two masses rigidly coupled: index reduction"
    parameter Real m1 = 1;
    parameter Real m2 = 3;
    parameter Real k = 10;
    input Real f;
    output Real p;
    Real x1(start = 0.5, fixed = true);
    Real v1(start = 0, fixed = true);
    Real x2;
    Real v2;
    Real fc;
  equation
    der(x1) = v1;
    der(x2) = v2;
    m1 * der(v1) = f - k * x1 + fc;
    m2 * der(v2) = -fc;
    x1 = x2;
    p = x2;
  end RigidMasses;

  model Pendulum "nonlinear pendulum with a torque input: linearized after a simulation"
    parameter Real L = 1;
    parameter Real g = 9.81;
    parameter Real d = 0.2;
    input Real tau;
    output Real angle;
    output Real height;
    Real phi(start = 0.5, fixed = true);
    Real w(start = 0, fixed = true);
  equation
    der(phi) = w;
    L ^ 2 * der(w) = -g * L * sin(phi) - d * w + tau;
    angle = phi;
    height = -L * cos(phi);
  end Pendulum;
end Lin;
