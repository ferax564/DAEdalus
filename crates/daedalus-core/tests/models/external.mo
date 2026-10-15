// External C functions of the Modelica Standard Library that DAEdalus implements
// natively (declared here as in the MSL, without needing the MSL itself).
package Ext
  function xorshift64star
    input Integer stateIn[2];
    output Real result;
    output Integer stateOut[2];
  external "C" ModelicaRandom_xorshift64star(stateIn, stateOut, result)
    annotation (Library="ModelicaExternalC");
  end xorshift64star;

  function xorshift128plus
    input Integer stateIn[4];
    output Real result;
    output Integer stateOut[4];
  external "C" ModelicaRandom_xorshift128plus(stateIn, stateOut, result)
    annotation (Library="ModelicaExternalC");
  end xorshift128plus;

  function xorshift1024star
    input Integer stateIn[33];
    output Real result;
    output Integer stateOut[33];
  external "C" ModelicaRandom_xorshift1024star(stateIn, stateOut, result)
    annotation (Library="ModelicaExternalC");
  end xorshift1024star;

  impure function setInternalState
    input Integer[33] rngState;
    input Integer id;
  external "C" ModelicaRandom_setInternalState_xorshift1024star(rngState, size(rngState, 1), id)
    annotation (Library="ModelicaExternalC");
  end setInternalState;

  impure function initializeImpureRandom
    input Integer seed;
    output Integer id;
  protected
    Integer rngState[33];
  algorithm
    rngState := testState();
    setInternalState(rngState, seed);
    id := seed;
  end initializeImpureRandom;

  impure function impureRandom
    input Integer id;
    output Real y;
  external "C" y = ModelicaRandom_impureRandom_xorshift1024star(id)
    annotation (Library="ModelicaExternalC");
  end impureRandom;

  function testState "s[i] = 1000*i - 7 + 12345*i^2 (i from 0), last element 0"
    output Integer s[33];
  algorithm
    for i in 1:32 loop
      s[i] := 1000*(i - 1) - 7 + (i - 1)*(i - 1)*12345;
    end for;
    s[33] := 0;
  end testState;

  function rawRealFFT
    input Real u[:];
    output Integer info;
    output Real amplitudes[div(size(u, 1), 2) + 1];
    output Real phases[div(size(u, 1), 2) + 1];
  protected
    Real work[3*size(u, 1) + 2*(div(size(u, 1), 2) + 1)];
  external "C" info = ModelicaFFT_kiss_fftr(u, size(u, 1), work, size(work, 1), amplitudes, phases)
    annotation (Library="ModelicaExternalC");
  end rawRealFFT;

  function fftAmplitude
    input Real u[:];
    input Integer k;
    output Real a;
  protected
    Integer info;
    Real A[div(size(u, 1), 2) + 1];
    Real P[div(size(u, 1), 2) + 1];
  algorithm
    (info, A, P) := rawRealFFT(u);
    a := A[k];
  end fftAmplitude;

  impure function print
    input String string = "";
    input String fileName = "";
  external "C" ModelicaInternal_print(string, fileName)
    annotation (Library="ModelicaExternalC");
  end print;

  function scanReal
    input String string;
    input Integer startIndex = 1;
    input Boolean unsigned = false;
    output Integer nextIndex;
    output Real number;
  external "C" ModelicaStrings_scanReal(string, startIndex, unsigned, nextIndex, number)
    annotation (Library="ModelicaExternalC");
  end scanReal;

  function scanInteger
    input String string;
    input Integer startIndex = 1;
    input Boolean unsigned = false;
    output Integer nextIndex;
    output Integer number;
  external "C" ModelicaStrings_scanInteger(string, startIndex, unsigned, nextIndex, number)
    annotation (Library="ModelicaExternalC");
  end scanInteger;

  function secondNumber "The second number of a line such as \"x = 1.5, 2e3\""
    input String line;
    output Real y;
  protected
    Integer next;
    Real first;
  algorithm
    (next, first) := scanReal(line, 4);
    (next, y) := scanReal(line, next + 1);
  end secondNumber;

  // Pure generators at translation time and at events.
  model Generators
    parameter Real y64 = xorshift64star({1, 2});
    parameter Real y128 = xorshift128plus({1, 2, 3, 4});
    parameter Real y1024 = xorshift1024star(testState());
    discrete Integer s[2](start = {1, 2}, each fixed = true);
    discrete Real y(start = 0, fixed = true);
    discrete Integer n(start = 0, fixed = true);
  algorithm
    when sample(0.1, 0.1) then
      (y, s) := xorshift64star(pre(s));
      n := pre(n) + 1;
    end when;
  end Generators;

  // An impure generator: every call gives the next number, in order.
  model Impure
    parameter Integer id = initializeImpureRandom(5);
    discrete Real r1(start = 0, fixed = true);
    discrete Real r2(start = 0, fixed = true);
  algorithm
    when sample(0.1, 0.1) then
      r1 := impureRandom(id);
      r2 := impureRandom(id);
      print("drawn");
    end when;
    when terminal() then
      print("done");
    end when;
  end Impure;

  // FFT of 1 + 2*cos(w*k) + 0.5*sin(2*w*k), w = 2*pi/8, scaled by time.
  model FFT
    constant Real pi = 3.14159265358979323846;
    parameter Real u[8] = {1 + 2*cos(2*pi*k/8) + 0.5*sin(4*pi*k/8) for k in 0:7};
    Real amp[5];
    Real phi[5];
    Integer info;
    parameter Real a2 = fftAmplitude(u, 2);
  equation
    (info, amp, phi) = rawRealFFT(time*u);
  end FFT;

  model Scan
    parameter Real x = secondNumber("x = 1.5, 2e3 // comment");
    parameter Integer k = scanInteger(" 42;");
  end Scan;
end Ext;
