// State machines (MLS chapter 17).
package SM

  // ModelicaReference.StateMachines: no clock, so the default clock of 1 s.
  model Reference
    inner Integer v(start = 0);
    block Increase
      outer output Integer v;
    equation
      v = previous(v) + 2;
    end Increase;
    Increase increase;
    block Decrease
      outer output Integer v;
    equation
      v = previous(v) - 1;
    end Decrease;
    Decrease decrease;
  equation
    initialState(increase);
    transition(increase, decrease, v >= 6, immediate = false);
    transition(decrease, increase, v == 0, immediate = false);
  end Reference;

  // Two states on Clock(0.1); immediate vs. delayed entry, reset vs. no reset of the
  // local counter, and the counters behind ticksInState().
  model TwoStates
    parameter Boolean immediate = true;
    parameter Boolean reset = true;
    inner Integer y(start = 0);
    Integer k(start = 0);
    Integer z;
    block A
      outer output Integer y;
      Integer n(start = 0);
    equation
      n = previous(n) + 1;
      y = n;
    end A;
    block B
      outer output Integer y;
    equation
      y = -1;
    end B;
    A a;
    B b;
  equation
    when Clock(0.1) then
      k = previous(k) + 1;
    end when;
    // Links the machine to the clock of k.
    z = if activeState(b) then k else 0;
    initialState(a);
    transition(a, b, ticksInState() >= 3, immediate = immediate, reset = reset);
    transition(b, a, ticksInState() >= 2, immediate = false, reset = reset);
  end TwoStates;
  model TwoStatesNoReset = TwoStates(reset = false);
  model TwoStatesDelayed = TwoStates(immediate = false);

  // Priorities: from s0 both transitions are enabled at the first tick; the one with
  // priority 1 wins.
  model Priorities
    inner Integer y(start = 0);
    block S
      parameter Integer id;
      outer output Integer y;
    equation
      y = id;
    end S;
    S s0(id = 10);
    S s1(id = 11);
    S s2(id = 12);
    Integer t(start = 0);
  equation
    t = previous(t) + 1;
    initialState(s0);
    transition(s0, s1, t >= 2, immediate = false, priority = 2);
    transition(s0, s2, t >= 3, immediate = false, priority = 1);
    transition(s1, s0, true, immediate = false);
    transition(s2, s0, true, immediate = false);
  end Priorities;

  // timeInState() on an explicit Clock(0.25), inferred through x.
  model Timed
    inner Integer y(start = 0);
    Integer x(start = 0);
    block A
      outer output Integer y;
    equation
      y = 1;
    end A;
    block B
      outer output Integer y;
    equation
      y = 2;
    end B;
    A a;
    B b;
  equation
    when Clock(0.25) then
      x = previous(x) + 1;
    end when;
    initialState(a);
    transition(a, b, timeInState() >= 0.5 and x > 0, immediate = false);
    transition(b, a, timeInState() >= 0.25, immediate = false);
  end Timed;

  // synchronize = true: p is left only once its inner machine reached its final state.
  model Sync
    inner Integer y(start = 0);
    block P
      outer output Integer y;
      block Q1
        outer output Integer y;
      equation
        y = 1;
      end Q1;
      block Q2
        outer output Integer y;
      equation
        y = 2;
      end Q2;
      Q1 q1;
      Q2 q2;
    equation
      initialState(q1);
      transition(q1, q2, ticksInState() >= 2, immediate = false);
    end P;
    block R
      outer output Integer y;
    equation
      y = 3;
    end R;
    P p;
    R r;
  equation
    initialState(p);
    transition(p, r, true, immediate = false, synchronize = true);
    transition(r, p, true, immediate = false);
  end Sync;

  // Hierarchical machine with a nested machine inside `outer1`, entered with and
  // without reset.
  model Hierarchical
    parameter Boolean reset = true;
    inner Integer v(start = 0);
    block Inner1
      outer output Integer v;
      Integer c(start = 0);
      block Up
        outer output Integer v;
      equation
        v = previous(v) + 1;
      end Up;
      block Hold
        outer output Integer v;
      equation
        v = previous(v);
      end Hold;
      Up up;
      Hold hold;
    equation
      c = previous(c) + 1;
      initialState(up);
      transition(up, hold, v >= 2, immediate = false);
    end Inner1;
    block Other
      outer output Integer v;
    equation
      v = 100;
    end Other;
    Inner1 outer1;
    Other other;
  equation
    initialState(outer1);
    transition(outer1, other, outer1.c >= 4, immediate = false);
    transition(other, outer1, true, immediate = false, reset = reset);
  end Hierarchical;
  model HierarchicalNoReset = Hierarchical(reset = false);

  // MLS 17.4 example (hierarchical and parallel state machines).
  model HierarchicalAndParallel
    inner Integer v(start = 0);
    State1 state1;
    State2 state2;
  equation
    initialState(state1);
    transition(state1, state2, activeState(state1.stateD) and activeState(state1.stateY),
      immediate = false);
    transition(state2, state1, v >= 20, immediate = false);
  public
    block State1
      inner Integer count(start = 0);
      inner outer output Integer v;
      block StateA
        outer output Integer v;
      equation
        v = previous(v) + 2;
      end StateA;
      StateA stateA;
      block StateB
        outer output Integer v;
      equation
        v = previous(v) - 1;
      end StateB;
      StateB stateB;
      block StateC
        outer output Integer count;
      equation
        count = previous(count) + 1;
      end StateC;
      StateC stateC;
      block StateD
      end StateD;
      StateD stateD;
    equation
      initialState(stateA);
      transition(stateA, stateB, v >= 6, immediate = false);
      transition(stateB, stateC, v == 0, immediate = false);
      transition(stateC, stateA, true, immediate = false, priority = 2);
      transition(stateC, stateD, count >= 2, immediate = false);
    public
      block StateX
        outer input Integer v;
        Integer i(start = 0);
        Integer w;
      equation
        i = previous(i) + 1;
        w = v;
      end StateX;
      StateX stateX;
      block StateY
        Integer j(start = 0);
      equation
        j = previous(j) + 1;
      end StateY;
      StateY stateY;
    equation
      initialState(stateX);
      transition(stateX, stateY, stateX.i > 20, immediate = false, reset = false);
    end State1;
    block State2
      outer output Integer v;
    equation
      v = previous(v) + 5;
    end State2;
  end HierarchicalAndParallel;

  // Rejected: the immediate transition reads a.n, which a computes in the same tick.
  model ImmediateLoop
    inner Integer y(start = 0);
    block A
      outer output Integer y;
      Integer n(start = 0);
    equation
      n = previous(n) + 1;
      y = n;
    end A;
    block B
      outer output Integer y;
    equation
      y = -1;
    end B;
    A a;
    B b;
  equation
    initialState(a);
    transition(a, b, a.n >= 3);
    transition(b, a, true, immediate = false);
  end ImmediateLoop;

  // Rejected: ticksInState() outside a transition condition.
  model TicksInStateInState
    inner Integer y(start = 0);
    block A
      outer output Integer y;
    equation
      y = ticksInState();
    end A;
    A a;
  equation
    initialState(a);
  end TicksInStateInState;

  // Rejected: two transitions from a with the same priority.
  model SamePriority
    block A
      Integer n(start = 0);
    equation
      n = previous(n) + 1;
    end A;
    A a, b, c;
  equation
    initialState(a);
    transition(a, b, true);
    transition(a, c, true);
  end SamePriority;
end SM;
