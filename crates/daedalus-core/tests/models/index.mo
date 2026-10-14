package T
model Pendulum
  parameter Real L = 1, m = 1, g = 9.81;
  Real x(start = 1), y(start = 0), vx, vy, F;
equation
  der(x) = vx; der(y) = vy;
  m*der(vx) = -F*x/L;
  m*der(vy) = -F*y/L - m*g;
  x^2 + y^2 = L^2;
end Pendulum;
model TwoInertias
  parameter Real J1 = 1, J2 = 2, ratio = 3;
  Real phi1(start=0, fixed=true), w1(start=0, fixed=true), phi2, w2, tau;
equation
  der(phi1) = w1; der(phi2) = w2;
  J1*der(w1) = 10 - ratio*tau;
  J2*der(w2) = tau;
  phi1 = ratio*phi2;
end TwoInertias;
model Loop
  Real x(start=1), y, z;
equation
  der(x) = -y;
  y = z + sin(y) * 0.1 + x;
  z = 0.5*y - x*z;
end Loop;
end T;
