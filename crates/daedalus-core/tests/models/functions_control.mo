model FnTest
  function firstIdx
    input Boolean b[:];
    output Integer index;
  algorithm
    index := 0;
    for i in 1:size(b,1) loop
      if b[i] then
        index := i;
        return;
      end if;
    end for;
  end firstIdx;
  function staticBreak
    input Real x;
    output Real y;
  algorithm
    y := 0;
    for i in 1:10 loop
      if i > 3 then
        break;
      end if;
      y := y + x;
    end for;
  end staticBreak;
  function dynBreak
    input Real x;
    output Real y;
  algorithm
    y := 0;
    for i in 1:10 loop
      if y > x then
        break;
      end if;
      y := y + 1;
    end for;
  end dynBreak;
  function earlyRet
    input Real x;
    output Real y;
  algorithm
    y := 1;
    if x > 0.5 then
      y := 2;
      return;
    elseif x > 0.25 then
      y := 3;
    end if;
    y := y * 10;
  end earlyRet;
  Integer k = firstIdx({time > 0.3, time > 0.6, true});
  Real s = staticBreak(2);
  Real d = dynBreak(10*time);
  Real r = earlyRet(time);
  annotation(experiment(StopTime=1));
end FnTest;
