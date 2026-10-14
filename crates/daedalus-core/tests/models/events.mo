package E
model Ball
  parameter Real e = 0.8, g = 9.81;
  Real h(start = 1, fixed = true), v(start = 0, fixed = true);
  Integer bounces(start = 0);
equation
  der(h) = v;
  der(v) = -g;
  when h < 0 then
    reinit(v, -e*pre(v));
    bounces = pre(bounces) + 1;
  end when;
end Ball;
model Sampled
  Real x(start = 0, fixed = true);
  discrete Real u(start = 0);
equation
  der(x) = u - x;
  when sample(0, 0.1) then
    u = if time < 0.5 then 1 else -1;
  end when;
end Sampled;
model Hyst
  Real x(start=0, fixed=true);
  Boolean on(start=true);
equation
  der(x) = if on then 1 else -1;
  when x > 1 then on = false; elsewhen x < 0 then on = true; end when;
end Hyst;
end E;
