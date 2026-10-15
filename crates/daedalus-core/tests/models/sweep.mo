package Sweep
  model Decay "n copies of x' = -k x, x(0) = x0: y = n x0 exp(-k t)"
    parameter Real k = 1 "decay rate";
    parameter Real x0 = 1 "initial value";
    parameter Integer n = 2 "number of copies (structural: sizes an array)";
    parameter Real k2 = 2 * k "computed from k";
    Real x[n](each start = x0, each fixed = true);
    Real y;
    Real z(start = 0, fixed = true) "integral of k2: z = 2 k t";
  equation
    for i in 1:n loop
      der(x[i]) = -k * x[i];
    end for;
    y = sum(x);
    der(z) = k2;
    annotation(experiment(StopTime = 1));
  end Decay;
end Sweep;
