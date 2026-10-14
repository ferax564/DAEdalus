package Lib
connector Flange
  Real s;
  flow Real f;
end Flange;
partial model TwoFlanges
  Flange a; Flange b;
end TwoFlanges;
model Mass
  extends TwoFlanges;
  parameter Real m = 1;
  Real s(start = 1, fixed=true);
  Real v(start = 0);
equation
  a.s = s; b.s = s;
  v = der(s);
  m*der(v) = a.f + b.f;
end Mass;
model Spring
  extends TwoFlanges;
  parameter Real c = 10;
  parameter Real s0 = 0;
equation
  a.f = -b.f;
  b.f = c*(b.s - a.s - s0);
end Spring;
model Fixed
  Flange a;
  parameter Real s0 = 0;
equation
  a.s = s0;
end Fixed;
model Test
  Mass mass(m=2);
  Spring spring(c=100);
  Fixed fixed;
equation
  connect(fixed.a, spring.a);
  connect(spring.b, mass.a);
  when mass.s > 1.5 then
    reinit(mass.v, 0);
  end when;
end Test;
end Lib;
