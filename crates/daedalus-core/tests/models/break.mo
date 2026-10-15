// `break` under a time-varying condition inside a function's for-loop.
package B
  function leadingSum "Sum of the leading elements of x not above lim"
    input Real x[:];
    input Real lim;
    output Real s;
  algorithm
    s := 0;
    for i in 1:size(x, 1) loop
      if x[i] > lim then
        break;
      end if;
      s := s + x[i];
    end for;
  end leadingSum;

  model Test
    Real y = leadingSum({1, 2, 3, 4}, 4*time);
  end Test;
end B;
