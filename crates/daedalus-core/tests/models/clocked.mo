// Synchronous language elements: a sampled first-order plant with a held
// proportional controller, a tick counter and a sub-sampled counter.
model Clocked
  Real x(start = 1, fixed = true);
  Real u;
  Real xd(start = 0);
  Real k(start = 0);
  Real ks(start = 0);
  Real kk;
  Real ksh;
equation
  der(x) = -x + u;
  when Clock(0.1) then
    xd = sample(x);
    k = previous(k) + 1;
  end when;
  kk = subSample(k, 3);
  ks = if kk >= 0 then previous(ks) + 1 else 0;
  u = hold(-2*xd);
  ksh = hold(ks);
end Clocked;
