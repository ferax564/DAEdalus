// Test problems for the solver suite.
package S
  // Robertson's chemical kinetics (Hairer & Wanner, Solving ODEs II, IV.1).
  model Robertson
    Real y1(start = 1, fixed = true), y2(start = 0, fixed = true), y3(start = 0, fixed = true);
  equation
    der(y1) = -0.04*y1 + 1e4*y2*y3;
    der(y2) = 0.04*y1 - 1e4*y2*y3 - 3e7*y2^2;
    der(y3) = 3e7*y2^2;
  end Robertson;

  // Van der Pol with mu = 1000 in Hairer's scaling (time t/mu, eps = 1/mu^2): the
  // VDPOL problem of the Hairer & Wanner test set.
  model VanDerPol
    parameter Real eps = 1e-6;
    Real y1(start = 2, fixed = true), y2(start = -0.66, fixed = true);
  equation
    der(y1) = y2;
    der(y2) = ((1 - y1^2)*y2 - y1)/eps;
  end VanDerPol;

  // Damped rotation: x = exp(-t/2) cos t, y = -exp(-t/2) sin t.
  model Linear
    Real x(start = 1, fixed = true), y(start = 0, fixed = true);
  equation
    der(x) = -0.5*x + y;
    der(y) = -x - 0.5*y;
  end Linear;

  // Prothero-Robinson: x = cos t for x(0) = 1, however stiff.
  model Prothero
    parameter Real lambda = -1e6;
    Real x(start = 1, fixed = true);
  equation
    der(x) = lambda*(x - cos(time)) - sin(time);
  end Prothero;

  // Stiff heat conduction in 60 cells (sparse Jacobian, eigenvalues down to -4000);
  // the steady state is linear, T[i] = 1 - i/(N+1).
  model Heat
    parameter Integer N = 60;
    parameter Real k = 1000;
    Real T[N](each start = 0, each fixed = true);
  equation
    der(T[1]) = k*(1 - 2*T[1] + T[2]);
    for i in 2:N - 1 loop
      der(T[i]) = k*(T[i - 1] - 2*T[i] + T[i + 1]);
    end for;
    der(T[N]) = k*(T[N - 1] - 2*T[N]);
  end Heat;

  // Nonlinear states coupled through an algebraic loop (for the Jacobian test).
  model Loop
    Real x1(start = 0.3, fixed = true), x2(start = -0.2, fixed = true), z;
  equation
    z + 0.3*sin(z) = x1*x2 + x1^2;
    der(x1) = -z*exp(x2) + cos(x1);
    der(x2) = x1*z - x2^3;
  end Loop;

  // Non-stiff, then stiff between t = 5 and t = 15 (k up to 1e5), then non-stiff
  // again: x follows sin(time) closely while k is large.
  model Switching
    Real k = 1 + 0.5e5*(tanh(10*(time - 5)) - tanh(10*(time - 15)));
    Real x(start = 0, fixed = true), v(start = 1, fixed = true);
  equation
    der(x) = v - k*(x - sin(time));
    der(v) = -x;
  end Switching;
end S;
