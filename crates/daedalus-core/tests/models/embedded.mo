package Emb "Models for the embedded C generator tests"
  model Loop "Nonlinear algebraic loop driven by an input"
    input Real u(start=0.3);
    output Real y;
    Real x(start=0.5);
  equation
    x = 0.5*cos(x) + u*y;
    y = sin(x) + 0.1*x;
  end Loop;

  model Ball "State events with reinit"
    parameter Real e=0.8;
    output Real h;
    Real v;
  initial equation
    h = 1;
    v = 0;
  equation
    der(h) = v;
    der(v) = -9.81;
    when h < 0 then
      reinit(v, -e*pre(v));
    end when;
  end Ball;

  model Mixed "Lookup table, Integer counter, Boolean, sampled update"
    input Real u(start=2);
    output Real y;
    output Real ny;
    output Real onv;
    Integer n(start=0, fixed=true);
    Boolean on;
    discrete Real held(start=0, fixed=true);
    Daedalus.Blocks.Sources.TimeTable tt(table=[0, 0; 0.5, 1; 1, 0.5; 2, 0.5]);
  equation
    y = tt.y*u + held;
    on = y > 1.25;
    when sample(0, 0.1) then
      n = pre(n) + 1;
      held = 0.01*n;
    end when;
    ny = n;
    onv = if on then 1 else 0;
  end Mixed;

  model Lag "Fixed-step integration of a stiff-free linear system with a parameter"
    parameter Real T=0.2;
    input Real u(start=1);
    output Real y(start=0, fixed=true);
    Real z(start=0, fixed=true);
  equation
    T*der(y) = z - y;
    T*der(z) = u - z;
  end Lag;
end Emb;
