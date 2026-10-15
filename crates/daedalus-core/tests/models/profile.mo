package Prof
  model TwoScales "Two decoupled states with time constants 1 ms and 1 s"
    Real fast(start = 1, fixed = true);
    Real slow(start = 1, fixed = true);
  equation
    der(fast) = -1000*fast;
    der(slow) = -slow;
  end TwoScales;

  model Chatter "Sliding on the switching surface x = 0"
    Real x(start = 1, fixed = true);
  equation
    der(x) = if x > 0 then -1 else 1;
  end Chatter;
end Prof;
