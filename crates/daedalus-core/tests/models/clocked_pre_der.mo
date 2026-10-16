model ClockedPreDer "Reduced clocked partition from ControlledMixingUnit"
  // A clocked state updated from its own previous derivative, the shape of
  // filter.x = pre(filter.x) + period * pre(der(filter.x)).
  Real x(start = 0, fixed = true);
equation
  when Clock(0.1) then
    x = pre(x) + 0.1 * pre(der(x));
  end when;
end ClockedPreDer;
