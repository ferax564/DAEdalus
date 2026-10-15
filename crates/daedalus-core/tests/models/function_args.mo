model FunctionArgs
  partial function Residual
    input Real u;
    output Real y;
  end Residual;
  function quadratic
    extends Residual;
    input Real p;
  algorithm
    y := u^2 - p;
  end quadratic;
  function bisect
    input Residual f;
    input Real lo;
    input Real hi;
    output Real u;
  protected
    Real a = lo;
    Real b = hi;
    Real m;
  algorithm
    while b - a > 1e-12 loop
      m := (a + b)/2;
      if f(m)*f(a) <= 0 then
        b := m;
      else
        a := m;
      end if;
    end while;
    u := (a + b)/2;
  end bisect;
  parameter Real p0 = 2;
  Real fixedRoot = bisect(function quadratic(p=p0), 0, 5);
  Real movingRoot = bisect(function quadratic(p=1 + time), 0, 5);
  String msg = "root " + String(p0);
end FunctionArgs;
