package Debug "Small (partly faulty) models for the equation and initialization debugger"
  connector Pin
    Real v;
    flow Real i;
  end Pin;

  partial model TwoPin
    Pin p;
    Pin n;
    Real v;
    Real i;
  equation
    v = p.v - n.v;
    0 = p.i + n.i;
    i = p.i;
  end TwoPin;

  model Resistor
    extends TwoPin;
    parameter Real R = 1;
  equation
    v = R*i;
  end Resistor;

  model Capacitor
    extends TwoPin;
    parameter Real C = 1;
  equation
    C*der(v) = i;
  end Capacitor;

  model Diode "Exponential diode: a nonlinear loop with a resistor"
    extends TwoPin;
    parameter Real Is = 1e-9;
    parameter Real Vt = 0.025;
  equation
    i = Is*(exp(v/Vt) - 1);
  end Diode;

  model Source
    extends TwoPin;
    parameter Real V = 1;
  equation
    v = V;
  end Source;

  model Ground
    Pin p;
  equation
    p.v = 0;
  end Ground;

  model RC "A healthy circuit"
    Source src(V = 2);
    Resistor r(R = 10);
    Capacitor c(C = 0.1, v(start = 0.5, fixed = true));
    Ground g;
  equation
    connect(src.p, r.p);
    connect(r.n, c.p);
    connect(c.n, g.p);
    connect(src.n, g.p);
  end RC;

  model DiodeLoop "Torn nonlinear loop: resistor in series with a diode"
    Source src(V = 1);
    Resistor r(R = 100);
    Diode d;
    Ground g;
  equation
    connect(src.p, r.p);
    connect(r.n, d.p);
    connect(d.n, g.p);
    connect(src.n, g.p);
  end DiodeLoop;

  model Pendulum "Cartesian pendulum: index reduction and dummy derivatives"
    parameter Real L = 1;
    parameter Real g = 9.81;
    Real x(start = 1, fixed = true);
    Real y(start = 0);
    Real vx;
    Real vy;
    Real F;
  equation
    der(x) = vx;
    der(y) = vy;
    der(vx) = -F*x;
    der(vy) = -F*y - g;
    x^2 + y^2 = L^2;
  end Pendulum;

  model Body
    parameter Real x0 = 1;
    Real x(start = x0);
    Real y;
  end Body;

  model InitFail "No real solution: exp(x) + y^2 > 0 cannot be -1"
    Body b(x0 = 2);
    Real s(start = 0, fixed = true);
  equation
    b.x + b.y = 1;
    exp(b.x) + b.y^2 = -1;
    der(s) = b.x;
  end InitFail;

  model SimFail "x^2 = 1 - time has no real root after t = 1"
    Real s(start = 0, fixed = true);
    Real x(start = 1);
  equation
    x^2 + x = 2 - time;
    der(s) = x;
    annotation(experiment(StopTime = 4));
  end SimFail;

  model Leaky
    Real u;
    Real w "no equation for w";
  equation
    u = 1;
  end Leaky;

  model Underdetermined
    Leaky leak;
    Real z;
  equation
    der(z) = -z + leak.u;
  end Underdetermined;

  model Fixer
    Real a;
  equation
    a = 1;
  end Fixer;

  model Overdetermined
    Fixer f;
    Real z;
  equation
    f.a = 2;
    der(z) = f.a;
  end Overdetermined;

  model OpenPin "r2.n is left open; r3 is not connected at all"
    Source src(V = 1);
    Resistor r(R = 10);
    Resistor r2(R = 5);
    Resistor r3;
    Ground g;
  equation
    connect(src.p, r.p);
    connect(r.n, g.p);
    connect(src.n, g.p);
    connect(r2.p, src.p);
  end OpenPin;
end Debug;
