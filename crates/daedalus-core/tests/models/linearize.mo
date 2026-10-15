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

  class Table1D "the external object of the MSL CombiTable1D (implemented natively)"
    extends ExternalObject;
    function constructor
      input Real table[:, :];
      input Integer smoothness;
      output Table1D t;
    external "C" t = ModelicaStandardTables_CombiTable1D_init2(
      "NoName", "NoName", table, size(table, 1), size(table, 2), {2}, 1,
      smoothness, 2, 0);
    end constructor;
    function destructor
      input Table1D t;
    external "C" ModelicaStandardTables_CombiTable1D_close(t);
    end destructor;
  end Table1D;

  function tableValue
    input Table1D t;
    input Real u;
    output Real y;
  external "C" y = ModelicaStandardTables_CombiTable1D_getValue(t, 1, u);
  end tableValue;

  model CubicTable "second derivative of an Akima table output in the dynamics"
    parameter Table1D t = Table1D([0, 0; 1, 1; 2, 4; 3, 9; 4, 3; 5, 0], 2);
    input Real u(start = 0.4);
    output Real y;
    Real x1(start = 1.3, fixed = true);
    Real x2(start = 0.7, fixed = true);
    Real x3(start = 0.2, fixed = true);
    Real s;
    Real ds;
  equation
    s = tableValue(t, x1);
    ds = der(s);
    der(x1) = x2;
    der(x2) = u - x1;
    der(x3) = der(ds) - x3;
    y = ds;
  end CubicTable;
end Lin;
