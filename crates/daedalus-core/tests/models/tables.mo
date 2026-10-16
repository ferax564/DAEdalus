package TablesTest
  "Stand-ins for the MSL table blocks (the same external C functions, which DAEdalus implements natively)"

  function fullPathName
    input String name;
    output String fullName;
  external "C" fullName = ModelicaInternal_fullPathName(name);
  end fullPathName;

  function loadResource
    input String uri;
    output String fileReference;
  algorithm
    fileReference := fullPathName(uri);
  end loadResource;

  class ExternalCombiTable1D
    extends ExternalObject;
    function constructor
      input String tableName;
      input String fileName;
      input Real table[:, :];
      input Integer columns[:];
      input Integer smoothness;
      input Integer extrapolation = 2;
      output ExternalCombiTable1D externalCombiTable1D;
    external "C" externalCombiTable1D = ModelicaStandardTables_CombiTable1D_init2(
      fileName, tableName, table, size(table, 1), size(table, 2), columns,
      size(columns, 1), smoothness, extrapolation, 0);
    end constructor;
    function destructor
      input ExternalCombiTable1D externalCombiTable1D;
    external "C" ModelicaStandardTables_CombiTable1D_close(externalCombiTable1D);
    end destructor;
  end ExternalCombiTable1D;

  class ExternalCombiTable2D
    extends ExternalObject;
    function constructor
      input String tableName;
      input String fileName;
      input Real table[:, :];
      input Integer smoothness;
      input Integer extrapolation = 2;
      input String delimiter = ",";
      input Integer nHeaderLines = 0;
      output ExternalCombiTable2D externalCombiTable2D;
    external "C" externalCombiTable2D = ModelicaStandardTables_CombiTable2D_init3(
      fileName, tableName, table, size(table, 1), size(table, 2), smoothness,
      extrapolation, 0, delimiter, nHeaderLines);
    end constructor;
    function destructor
      input ExternalCombiTable2D externalCombiTable2D;
    external "C" ModelicaStandardTables_CombiTable2D_close(externalCombiTable2D);
    end destructor;
  end ExternalCombiTable2D;

  function getTable1DValue
    input ExternalCombiTable1D tableID;
    input Integer icol;
    input Real u;
    output Real y;
  external "C" y = ModelicaStandardTables_CombiTable1D_getValue(tableID, icol, u);
  end getTable1DValue;

  function getTable2DValue
    input ExternalCombiTable2D tableID;
    input Real u1;
    input Real u2;
    output Real y;
  external "C" y = ModelicaStandardTables_CombiTable2D_getValue(tableID, u1, u2);
  end getTable2DValue;

  function getTable2DDerValue
    input ExternalCombiTable2D tableID;
    input Real u1;
    input Real u2;
    input Real der_u1;
    input Real der_u2;
    output Real der_y;
  external "C" der_y = ModelicaStandardTables_CombiTable2D_getDerValue(tableID, u1, u2, der_u1, der_u2);
  end getTable2DDerValue;

  function getTable2DAbscissaUmin
    input ExternalCombiTable2D tableID;
    output Real uMin[2];
  external "C" ModelicaStandardTables_CombiTable2D_minimumAbscissa(tableID, uMin);
  end getTable2DAbscissaUmin;

  function getTable2DAbscissaUmax
    input ExternalCombiTable2D tableID;
    output Real uMax[2];
  external "C" ModelicaStandardTables_CombiTable2D_maximumAbscissa(tableID, uMax);
  end getTable2DAbscissaUmax;

  block Table2D "Like Modelica.Blocks.Tables.CombiTable2Ds"
    parameter Boolean tableOnFile = false;
    parameter Real table[:, :] = fill(0.0, 0, 2);
    parameter String tableName = "NoName";
    parameter String fileName = "NoName";
    parameter String delimiter = ",";
    parameter Integer nHeaderLines = 0;
    parameter Integer smoothness = 1 "1 linear, 2 Akima, 3 constant";
    parameter Integer extrapolation = 2 "1 hold, 2 last two points, 3 periodic";
    final parameter ExternalCombiTable2D tableID = ExternalCombiTable2D(
      if tableOnFile then tableName else "NoName",
      if tableOnFile then fileName else "NoName",
      table, smoothness, extrapolation, delimiter, nHeaderLines);
    final parameter Real u_min[2] = getTable2DAbscissaUmin(tableID);
    final parameter Real u_max[2] = getTable2DAbscissaUmax(tableID);
    input Real u1;
    input Real u2;
    output Real y;
  equation
    y = getTable2DValue(tableID, u1, u2);
  end Table2D;

  model Linear2D "Bilinear table, linear extrapolation in both directions"
    Table2D t(table = [0, 1, 2; 1, 1, 3; 2, 5, 8], u1 = 1 + time, u2 = 1 + 2*time);
    Real dy = der(t.y);
  end Linear2D;

  model Akima2D "MSL ModelicaTest.Tables.CombiTable2Ds.Test1 (u1 extrapolation) and Test2 (u2 extrapolation)"
    parameter Real table[:, :] = [0, 0, 5, 10, 15, 20, 25, 30, 35, 40; 0, 58.2,
      61.5, 47.9, 62.3, 34.6, 45.5, 38.2, 41.2, 41.7; 5, 37.2, 40, 27,
      41.3, 14.1, 24.5, 17.3, 20.2, 20.8; 10, 22.4, 22.5, 14.6, 22.5, 4.7,
      7.2, 1.8, 2.1, 2.1; 15, 21.8, 20.5, 12.8, 17.6, 5.8, 7.6, 0.8, 0.6,
      0.6; 20, 16.8, 14.4, 8.1, 6.9, 6.2, 0.6, 0.1, 0, 0; 25, 12, 8, 5.3,
      2.9, 0.6, 0, 0, 0, 0; 30, 7.4, 4.8, 1.4, 0.1, 0, 0, 0, 0, 0; 35,
      3.2, 0.7, 0, 0, 0, 0, 0, 0, 0; 40, 0, 0, 0, 0, 0, 0, 0, 0, 0; 45, 0,
      0, 0, 0, 0, 0, 0, 0, 0; 50, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    Table2D t1(table = table, smoothness = 2, u1 = time, u2 = 2);
    Table2D t2(table = table, smoothness = 2, u1 = 2, u2 = time);
    Real d1 = der(t1.y);
    Real dd1 = der(d1);
    Real d2 = der(t2.y);
    Real dd2 = der(d2);
  end Akima2D;

  model FileTables "Tables read from a text file and a CSV file"
    parameter String file = loadResource("modelica://TablesTest/tables.txt");
    Table2D t2(tableOnFile = true, tableName = "tab2", fileName = file, u1 = time, u2 = 2*time);
    Table2D tcsv(tableOnFile = true, tableName = "csv", delimiter = ";", nHeaderLines = 1,
      fileName = loadResource("modelica://TablesTest/tables2d.csv"), u1 = time, u2 = 2*time);
    parameter ExternalCombiTable1D t1 = ExternalCombiTable1D("tab1", file, fill(0.0, 0, 2), {2, 3}, 1);
    Real y1a = getTable1DValue(t1, 1, 2*time);
    Real y1b = getTable1DValue(t1, 2, 2*time);
  end FileTables;
  class ExternalCombiTimeTable
    extends ExternalObject;
    function constructor
      input String tableName;
      input String fileName;
      input Real table[:, :];
      input Real startTime;
      input Integer columns[:];
      input Integer smoothness;
      input Integer extrapolation;
      input Real shiftTime = 0;
      input Integer timeEvents = 1;
      output ExternalCombiTimeTable externalCombiTimeTable;
    external "C" externalCombiTimeTable = ModelicaStandardTables_CombiTimeTable_init3(
      fileName, tableName, table, size(table, 1), size(table, 2), startTime,
      columns, size(columns, 1), smoothness, extrapolation, shiftTime, timeEvents,
      0, ",", 0);
    end constructor;
    function destructor
      input ExternalCombiTimeTable externalCombiTimeTable;
    external "C" ModelicaStandardTables_CombiTimeTable_close(externalCombiTimeTable);
    end destructor;
  end ExternalCombiTimeTable;

  function getTimeTableValue
    input ExternalCombiTimeTable tableID;
    input Integer icol;
    input Real timeIn;
    input Real nextTimeEvent;
    input Real pre_nextTimeEvent;
    output Real y;
  external "C" y = ModelicaStandardTables_CombiTimeTable_getValue(tableID, icol, timeIn,
    nextTimeEvent, pre_nextTimeEvent);
  end getTimeTableValue;

  function getNextTimeEvent
    input ExternalCombiTimeTable tableID;
    input Real timeIn;
    output Real nextTimeEvent;
  external "C" nextTimeEvent = ModelicaStandardTables_CombiTimeTable_nextTimeEvent(tableID, timeIn);
  end getNextTimeEvent;

  block Table1D "Like Modelica.Blocks.Tables.CombiTable1Ds with one column"
    parameter Real table[:, :];
    parameter Integer smoothness = 1 "1 linear, 2 Akima, 3 constant, 4 Fritsch-Butland, 5 Steffen, 6 modified Akima";
    parameter Integer extrapolation = 2 "1 hold, 2 last two points, 3 periodic";
    final parameter ExternalCombiTable1D tableID = ExternalCombiTable1D(
      "NoName", "NoName", table, {2}, smoothness, extrapolation);
    input Real u;
    output Real y;
  equation
    y = getTable1DValue(tableID, 1, u);
  end Table1D;

  block TimeTable "Like Modelica.Blocks.Sources.CombiTimeTable with one column"
    parameter Real table[:, :];
    parameter Integer smoothness = 1;
    parameter Integer extrapolation = 2;
    parameter Real startTime = 0;
    final parameter ExternalCombiTimeTable tableID = ExternalCombiTimeTable(
      "NoName", "NoName", table, startTime, {2}, smoothness, extrapolation, startTime);
    discrete Real nextTimeEvent(start = 0, fixed = true);
    output Real y;
  equation
    when {time >= pre(nextTimeEvent), initial()} then
      nextTimeEvent = getNextTimeEvent(tableID, time);
    end when;
    y = getTimeTableValue(tableID, 1, time, nextTimeEvent, pre(nextTimeEvent));
  end TimeTable;

  model Monotone1D "MSL ModelicaTest.Tables.CombiTable1Ds.Test26 (Fritsch-Butland) and Test30 (Steffen)"
    Table1D fb(table = [0, 5; 1, 3; 2, 1; 3, 1; 4, 1; 5, 3], smoothness = 4, u = time);
    Table1D steffen(table = [0, 5; 1, 3; 2, 1; 3, 1; 4, 1; 5, 3], smoothness = 5, u = time);
    Real dfb = der(fb.y);
    Real d2fb = der(dfb);
    Real dsteffen = der(steffen.y);
    Real d2steffen = der(dsteffen);
  end Monotone1D;

  model PeriodicTime "MSL ModelicaTest.Tables.CombiTimeTable.Test1: periodic, with jumps"
    TimeTable t(table = [0.1, 0; 0.3, 0; 0.5, -1; 0.51, -2; 0.51, -3; 0.6, -2; 0.7, -2; 0.7, -2; 1, -4],
      extrapolation = 3);
    Real dy = der(t.y);
  end PeriodicTime;

  model DerOnGridLine "getDerValue on the grid line u1 = 1, where the slope in u1 changes"
    // z = 0, 1, 3 at u1 = 0, 1, 2 (independent of u2): slope 1 left of u1 = 1, 2 right
    parameter ExternalCombiTable2D tableID = ExternalCombiTable2D("NoName", "NoName",
      [0, 0, 1; 0, 0, 0; 1, 1, 1; 2, 3, 3], 1, 2);
    Real du = time - 0.5;
    Real dy = getTable2DDerValue(tableID, 1, 0.5, du, 0);
    Real dyConst = getTable2DDerValue(tableID, 1, 0.5, -1, 0);
  end DerOnGridLine;

  model NoExtrapolation2D "extrapolation = NoExtrapolation: u1 leaves [0, 1] at time 1"
    Table2D t(table = [0, 0, 1; 0, 0, 1; 1, 2, 3], extrapolation = 4, u1 = time, u2 = 0.5);
  end NoExtrapolation2D;

  model NoExtrapolation1D "extrapolation = NoExtrapolation: u leaves [0, 1] at time 1"
    parameter ExternalCombiTable1D tableID = ExternalCombiTable1D("NoName", "NoName",
      [0, 0; 1, 2], {2}, 1, 4);
    Real y = getTable1DValue(tableID, 1, time);
  end NoExtrapolation1D;
end TablesTest;
