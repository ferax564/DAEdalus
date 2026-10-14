within Daedalus;
package Vehicle "Longitudinal vehicle dynamics, electric and combustion powertrains, drivers and drive cycles"
  extends Daedalus.Icons.Package;
  import Daedalus.Mechanics.Rotational;

  package DriveCycles "Standard speed-time cycles (time [s], speed [km/h])"
    extends Daedalus.Icons.Package;

    constant Real NEDC[:, 2]=[
      0,0; 11,0; 15,15; 23,15; 28,0; 49,0; 61,32; 85,32; 96,0; 117,0;
      143,50; 155,50; 163,35; 176,35; 188,0; 195,0; 206,0; 210,15; 218,15; 223,0;
      244,0; 256,32; 280,32; 291,0; 312,0; 338,50; 350,50; 358,35; 371,35; 383,0;
      390,0; 401,0; 405,15; 413,15; 418,0; 439,0; 451,32; 475,32; 486,0; 507,0;
      533,50; 545,50; 553,35; 566,35; 578,0; 585,0; 596,0; 600,15; 608,15; 613,0;
      634,0; 646,32; 670,32; 681,0; 702,0; 728,50; 740,50; 748,35; 761,35; 773,0;
      780,0; 800,0; 821,70; 871,70; 879,50; 948,50; 961,70; 1011,70; 1046,100; 1076,100;
      1096,120; 1106,120; 1122,80; 1130,50; 1140,0; 1180,0]
      "New European Driving Cycle: 4 x ECE-15 urban + EUDC extra-urban, 1180 s. Piecewise-linear
       reconstruction from the regulation's segment table (UN ECE R101 / 70/220/EEC); gear-change
       pauses are merged into the adjacent ramps, which gives 10.70 km instead of the official 11.02 km.";

    constant Real UDDS[:, 2]=[
      0,0; 20,0; 21,4.83; 22,9.5; 23,13.84; 24,18.51; 25,23.01; 26,27.2; 27,27.84; 28,29.13;
      29,33.31; 30,34.92; 31,36.05; 32,36.21; 33,35.57; 34,34.6; 35,33.64; 36,32.83; 37,31.87; 38,27.36;
      39,23.98; 40,23.98; 41,24.46; 42,24.95; 43,25.75; 44,27.52; 46,33.96; 47,36.53; 48,36.85; 49,36.53;
      50,36.37; 51,34.28; 52,30.58; 53,27.52; 54,25.43; 55,25.43; 56,28.49; 57,31.87; 58,34.76; 59,37.34;
      60,38.95; 61,39.59; 62,40.07; 63,40.23; 64,39.59; 65,39.43; 66,39.75; 67,39.91; 69,39.59; 70,39.59;
      71,40.4; 72,41.2; 73,41.36; 74,40.88; 75,40.07; 76,40.23; 77,40.88; 78,41.84; 79,41.84; 80,41.36;
      81,42; 82,42.97; 83,44.26; 84,46.03; 85,47.15; 86,47.96; 88,48.92; 89,49.41; 90,49.41; 91,49.09;
      92,48.92; 93,48.76; 94,48.92; 95,49.57; 96,48.92; 97,48.12; 98,47.48; 99,47.96; 100,48.76; 101,49.41;
      102,49.73; 103,49.89; 104,49.73; 105,48.92; 106,47.96; 107,48.12; 108,48.6; 109,49.41; 110,50.21; 111,51.18;
      112,51.82; 113,52.14; 114,51.82; 115,51.02; 116,46.03; 119,30.1; 120,24.78; 124,3.54; 125,0; 163,0;
      168,26.55; 169,31.87; 170,35.73; 171,39.11; 172,41.52; 173,42.49; 174,41.36; 175,40.4; 176,39.75; 177,40.23;
      178,40.56; 179,40.88; 180,41.52; 181,43.77; 182,42.65; 183,38.62; 184,36.53; 185,31.22; 186,28.49; 187,27.68;
      188,29.13; 189,29.93; 190,32.19; 191,35.73; 192,39.43; 193,43.94; 194,49.09; 195,53.91; 196,58.26; 197,60.03;
      198,63.25; 199,65.18; 200,67.75; 201,70.01; 202,72.58; 203,74.03; 204,75.32; 205,76.45; 206,76.45; 207,76.12;
      208,75.96; 209,75.64; 213,75.64; 215,76.28; 216,77.09; 217,78.05; 218,79.02; 219,79.66; 220,80.47; 221,81.43;
      222,82.08; 223,82.88; 224,84.01; 225,85.62; 226,87.07; 227,87.87; 228,88.35; 229,88.52; 230,88.35; 231,87.87;
      232,87.87; 233,88.19; 234,88.68; 235,89.32; 236,89.64; 237,90.29; 238,90.61; 239,91.09; 240,91.25; 241,91.25;
      242,90.93; 247,90.93; 248,90.77; 249,90.29; 250,89.8; 251,88.68; 252,87.87; 253,87.23; 254,86.91; 255,86.42;
      256,86.26; 257,86.75; 259,87.07; 260,87.07; 261,86.58; 263,85.3; 264,84.65; 265,83.85; 266,84.33; 267,83.69;
      268,83.53; 269,83.2; 270,82.88; 271,83.04; 272,83.37; 273,83.85; 274,84.49; 275,85.3; 276,86.1; 277,86.91;
      278,88.35; 279,89.16; 280,89.48; 281,90.12; 282,90.12; 283,89.8; 284,88.84; 285,87.71; 286,86.26; 287,84.49;
      288,82.88; 290,82.88; 291,82.24; 292,80.63; 293,80.47; 294,80.63; 295,80.47; 296,79.82; 297,79.66; 299,79.66;
      300,79.02; 301,78.22; 302,77.41; 303,75.96; 305,72.42; 307,68.56; 308,66.79; 309,64.86; 310,61.96; 311,59.55;
      312,56.65; 313,54.4; 314,52.3; 315,50.7; 316,49.25; 317,49.09; 318,48.28; 319,46.67; 320,44.26; 321,39.91;
      322,34.6; 323,32.35; 324,30.74; 325,29.77; 327,24.95; 328,20.12; 329,17.38; 330,12.87; 332,2.25; 333,0;
      346,0; 347,1.61; 351,22.85; 352,27.84; 353,32.19; 354,36.21; 355,38.14; 356,40.56; 357,42.81; 358,45.22;
      359,48.28; 361,50.86; 362,51.66; 363,52.79; 364,54.07; 365,55.52; 366,55.68; 367,56.17; 368,56.01; 369,55.52;
      370,55.85; 371,57.13; 372,57.94; 377,57.94; 378,58.1; 379,58.58; 380,58.74; 381,58.58; 382,57.94; 383,56.49;
      384,54.88; 385,53.91; 386,50.53; 387,46.67; 388,41.36; 389,37.02; 390,32.67; 391,28.16; 392,23.34; 393,19.31;
      396,3.38; 397,0; 402,0; 403,4.18; 404,9.5; 409,36.05; 410,40.23; 411,41.2; 412,44.26; 413,46.67;
      414,48.28; 415,48.44; 416,48.28; 417,47.8; 418,47.15; 419,46.35; 420,45.06; 421,40.23; 428,3.06; 429,0;
      447,0; 452,26.55; 453,31.87; 455,42.49; 456,44.74; 457,46.83; 458,50.7; 459,53.11; 460,54.07; 461,56.01;
      462,56.49; 463,57.29; 464,58.1; 465,57.94; 467,58.26; 468,57.94; 469,57.45; 470,57.94; 471,57.94; 472,57.29;
      474,56.97; 475,56.65; 480,56.65; 481,56.33; 483,56.65; 484,57.13; 485,56.65; 486,56.33; 488,56.33; 489,56.01;
      490,55.68; 491,55.52; 492,53.91; 493,51.5; 494,48.44; 495,45.06; 496,41.04; 497,36.21; 498,31.87; 499,26.55;
      500,21.24; 501,16.58; 502,11.59; 503,6.44; 504,1.61; 505,0; 510,0; 511,1.93; 512,5.63; 513,8.85;
      514,10.46; 515,13.68; 516,15.45; 517,16.9; 518,19.15; 519,22.53; 520,25.75; 521,28.49; 522,30.58; 523,32.35;
      524,33.8; 526,37.02; 527,38.3; 528,39.43; 529,40.07; 530,40.23; 535,40.23; 536,41.2; 538,41.84; 540,40.56;
      541,40.23; 543,40.23; 544,39.27; 545,37.18; 546,31.87; 547,26.55; 552,0; 568,0; 571,15.93; 572,20.92;
      573,23.5; 574,25.75; 575,27.36; 577,27.36; 578,28.16; 579,28.49; 580,28.49; 581,28.16; 582,27.36; 583,27.2;
      584,26.72; 585,27.36; 586,27.52; 587,27.36; 588,26.72; 589,26.55; 590,26.55; 591,26.72; 592,27.36; 593,28.32;
      594,29.77; 595,30.9; 596,32.51; 597,33.8; 599,34.12; 600,34.76; 601,35.41; 602,36.05; 603,36.21; 605,36.21;
      606,36.53; 607,38.14; 608,40.4; 609,41.84; 610,42.65; 611,43.45; 612,42; 619,4.83; 620,0; 645,0;
      646,3.22; 647,7.24; 648,12.55; 649,16.42; 650,20.12; 651,22.53; 652,24.62; 653,28.16; 654,31.54; 655,33.8;
      656,35.73; 657,37.5; 658,39.43; 659,40.72; 660,41.2; 661,41.84; 662,42; 663,42.17; 664,42.17; 665,42.49;
      666,42.65; 667,42.65; 668,41.84; 669,41.04; 670,37.98; 671,34.44; 672,29.77; 673,26.39; 674,23.34; 677,9.33;
      678,5.63; 679,3.22; 680,0; 693,0; 694,2.25; 695,5.31; 696,7.08; 697,10.46; 698,14.81; 699,18.19;
      700,21.73; 701,23.5; 702,26.39; 703,26.88; 704,26.55; 705,26.55; 706,29.29; 707,30.9; 708,32.35; 709,34.6;
      710,36.21; 711,36.21; 712,35.57; 713,36.53; 714,37.5; 715,37.82; 716,36.21; 717,34.76; 718,32.99; 719,28.97;
      722,14.48; 723,9.98; 724,7.24; 725,4.83; 726,3.38; 727,0.8; 728,0.8; 729,5.15; 730,10.46; 731,15.45;
      732,20.12; 733,22.53; 735,28.97; 736,31.54; 737,34.6; 738,37.18; 739,39.43; 741,42.65; 742,43.61; 743,44.42;
      744,44.9; 745,45.55; 746,46.03; 747,46.03; 748,45.55; 749,45.38; 750,45.06; 751,44.26; 752,43.13; 753,41.04;
      755,34.6; 756,30.58; 757,26.55; 758,23.98; 759,20.12; 760,15.13; 762,4.83; 763,2.41; 764,2.41; 765,0.8;
      766,0; 767,4.83; 770,20.76; 771,25.43; 772,28.16; 773,29.61; 774,31.38; 775,33.31; 776,35.41; 777,37.34;
      778,40.23; 779,42.65; 780,44.26; 781,45.06; 782,45.55; 783,46.51; 785,46.51; 786,46.35; 787,45.87; 788,45.55;
      790,45.55; 791,45.38; 792,44.42; 793,44.26; 798,44.26; 799,44.42; 800,45.06; 801,45.87; 802,48.28; 805,53.11;
      806,53.11; 807,54.07; 808,54.72; 809,55.2; 810,55.04; 811,54.72; 812,54.72; 813,54.56; 814,54.07; 815,53.27;
      816,53.11; 817,52.3; 818,51.5; 819,51.34; 820,50.86; 821,50.7; 822,49.25; 823,48.28; 824,48.12; 827,48.12;
      828,47.64; 829,47.48; 830,47.48; 831,47.15; 832,46.51; 833,45.38; 834,44.58; 835,43.45; 836,41.04; 837,38.14;
      838,35.41; 839,32.99; 840,30.9; 841,30.9; 842,32.35; 843,33.64; 844,34.44; 845,35.41; 846,36.37; 847,37.34;
      848,38.62; 850,41.84; 851,42.81; 852,42.81; 855,43.77; 856,44.74; 857,45.22; 858,46.35; 861,46.83; 862,46.67;
      863,45.22; 864,44.26; 865,43.45; 866,41.52; 867,40.23; 868,39.43; 869,39.91; 870,40.4; 871,41.04; 872,41.36;
      873,42.17; 874,43.29; 875,44.26; 876,44.74; 877,45.71; 878,46.67; 879,46.99; 882,46.51; 883,45.87; 884,45.22;
      885,45.06; 886,45.06; 887,44.42; 888,43.77; 889,42.81; 890,43.45; 891,44.26; 892,44.74; 893,45.06; 894,44.74;
      895,45.06; 897,45.06; 899,44.1; 900,43.29; 901,42.81; 902,42.65; 904,42.65; 905,42.33; 906,42.17; 907,42.17;
      908,41.68; 909,41.2; 910,41.2; 911,41.68; 912,41.52; 913,41.04; 914,39.59; 915,37.82; 916,35.73; 917,34.76;
      918,34.76; 919,34.92; 920,36.37; 921,37.66; 922,38.62; 923,38.95; 924,39.27; 925,40.07; 926,40.4; 928,40.72;
      929,41.04; 930,40.56; 931,40.23; 933,40.23; 934,39.75; 936,39.11; 937,39.11; 938,39.43; 939,40.23; 940,40.23;
      941,39.59; 942,39.59; 943,38.79; 944,39.43; 945,40.4; 946,41.2; 947,40.4; 948,38.62; 949,35.41; 950,32.35;
      951,27.2; 954,11.27; 955,5.95; 956,0.64; 957,0; 959,0; 960,3.22; 964,24.46; 965,28.16; 966,29.93;
      967,32.19; 968,33.96; 969,35.41; 970,37.02; 971,39.43; 972,42.33; 973,44.26; 974,45.22; 975,45.71; 976,45.87;
      978,45.87; 979,44.58; 980,44.26; 981,43.77; 982,43.13; 983,42.65; 984,41.84; 985,41.36; 986,40.56; 987,38.62;
      988,35.41; 989,34.6; 990,34.6; 991,35.08; 992,36.21; 993,37.02; 994,36.69; 995,36.69; 996,37.02; 997,36.53;
      999,36.53; 1000,37.82; 1001,38.62; 1002,39.59; 1003,39.91; 1004,40.4; 1005,41.04; 1006,41.2; 1007,41.04; 1008,40.23;
      1009,38.79; 1010,38.14; 1011,37.34; 1012,36.85; 1013,36.21; 1014,35.41; 1015,34.76; 1016,32.99; 1017,28.16; 1022,1.61;
      1023,0; 1052,0; 1053,1.93; 1054,6.44; 1057,22.37; 1058,27.36; 1059,29.77; 1060,32.19; 1061,35.08; 1062,37.02;
      1063,38.62; 1065,41.2; 1066,42.65; 1067,43.13; 1068,44.1; 1069,44.9; 1070,45.55; 1071,45.06; 1072,44.26; 1073,43.45;
      1074,43.45; 1075,42.33; 1076,39.43; 1077,36.21; 1078,34.6; 1079,33.15; 1080,28.97; 1081,24.14; 1082,19.8; 1083,17.86;
      1084,17.06; 1085,16.09; 1086,15.29; 1087,14.65; 1088,14; 1089,13.84; 1091,14.48; 1092,14; 1093,13.84; 1094,12.87;
      1095,11.27; 1096,8.05; 1097,6.76; 1098,4.18; 1099,1.61; 1100,0; 1101,0.16; 1102,0.97; 1103,2.57; 1104,5.79;
      1105,11.1; 1106,16.09; 1107,20.6; 1108,22.53; 1109,23.34; 1110,25.75; 1111,29.13; 1112,32.19; 1113,33.8; 1114,34.12;
      1116,34.44; 1117,34.92; 1118,36.21; 1119,37.02; 1120,38.3; 1121,39.43; 1122,40.23; 1124,39.91; 1125,40.23; 1126,40.88;
      1127,41.52; 1128,41.84; 1129,42.49; 1130,42.81; 1131,43.29; 1132,43.45; 1134,43.45; 1136,43.13; 1137,43.13; 1138,42.65;
      1139,42.49; 1140,41.84; 1141,41.04; 1142,39.59; 1143,37.82; 1144,34.6; 1145,32.19; 1146,28.16; 1147,25.75; 1148,22.53;
      1152,1.29; 1153,0; 1168,0; 1169,3.38; 1174,29.93; 1175,33.96; 1176,37.02; 1177,37.82; 1178,37.02; 1179,36.21;
      1180,32.19; 1182,21.57; 1183,16.25; 1186,0.32; 1187,0; 1196,0; 1197,0.32; 1198,2.41; 1199,5.63; 1200,10.46;
      1201,15.77; 1202,19.31; 1203,20.76; 1204,20.92; 1205,20.28; 1206,20.6; 1207,21.08; 1208,21.08; 1209,22.53; 1210,24.95;
      1211,27.36; 1212,29.93; 1213,31.7; 1214,33.8; 1215,34.6; 1216,35.08; 1217,35.08; 1219,34.12; 1221,35.08; 1222,35.41;
      1223,35.25; 1224,34.92; 1225,34.6; 1226,34.6; 1227,34.44; 1228,32.35; 1229,31.38; 1230,30.9; 1231,31.54; 1232,31.87;
      1233,32.19; 1234,31.38; 1235,28.16; 1236,24.95; 1237,20.92; 1238,16.09; 1239,12.87; 1240,9.66; 1241,6.44; 1242,4.02;
      1243,1.13; 1244,0; 1251,0; 1252,1.61; 1256,1.61; 1257,2.57; 1258,4.83; 1260,8.05; 1261,10.14; 1262,12.87;
      1263,16.09; 1264,16.9; 1266,13.68; 1267,12.23; 1268,14.16; 1269,17.7; 1271,27.36; 1272,31.38; 1273,33.8; 1274,35.08;
      1275,35.73; 1276,37.02; 1277,37.98; 1278,38.79; 1279,39.43; 1280,39.43; 1281,38.62; 1282,37.82; 1287,37.82; 1288,38.62;
      1289,38.79; 1290,39.43; 1291,39.75; 1292,40.23; 1293,40.88; 1294,41.2; 1295,41.36; 1296,41.84; 1297,42.17; 1298,43.45;
      1299,44.74; 1300,45.55; 1301,46.67; 1302,46.83; 1303,46.67; 1304,45.06; 1311,7.89; 1312,2.57; 1313,0; 1337,0;
      1338,2.41; 1339,7.72; 1340,13.04; 1341,18.35; 1342,21.24; 1343,24.3; 1344,27.04; 1345,29.45; 1346,31.38; 1347,32.67;
      1348,34.28; 1349,35.25; 1350,35.57; 1351,36.05; 1352,35.41; 1353,34.76; 1354,33.96; 1355,32.99; 1356,32.19; 1357,31.54;
      1358,29.77; 1360,26.55; 1361,24.95; 1362,22.53; 1364,12.87; 1365,8.37; 1366,4.02; 1367,0; 1369,0]
      "EPA Urban Dynamometer Driving Schedule (FTP-72), 1369 s, 11.99 km. Data: US EPA, as distributed
       with NREL FASTSim (fastsim-2 branch, resources/cycles/udds.csv); collinear points removed.";

    block DriveCycle "Reference speed from a drive cycle table, optionally repeated"
      extends Daedalus.Blocks.Interfaces.SO(y(unit="m/s"));
      parameter Real table[:, 2]=NEDC "Cycle [time s, speed km/h]";
      parameter Integer repetitions(min=1)=1 "Number of consecutive repetitions";
      parameter Units.Time T_cycle=table[size(table, 1), 1] "Cycle duration";
      Real v_kmh(unit="km/h") "Reference speed in km/h";
    protected
      Units.Time t_local;
    equation
      t_local = if time < repetitions*T_cycle then time - T_cycle*floor(time/T_cycle) else T_cycle;
      v_kmh = Daedalus.Blocks.Functions.interpolate(table, t_local, 2);
      y = v_kmh/3.6;
      annotation (Icon(graphics={Line(points={{-80,-60},{-60,-60},{-50,-20},{-30,-20},{-20,-60},{0,-60},{20,40},{50,40},{70,-60},{80,-60}}, color={0,0,127}),
          Text(extent={{-100,90},{100,60}}, textString="cycle")}));
    end DriveCycle;
  end DriveCycles;

  package Interfaces "Partial vehicle models"
    extends Daedalus.Icons.Package;

    partial model PartialVehicle "Point-mass longitudinal vehicle driven through the wheel axle"
      parameter Units.Mass m=1600 "Vehicle mass (incl. payload)";
      parameter Units.Inertia J_wheels=4*1.2 "Total rotational inertia of the wheels (adds to the effective mass)";
      parameter Units.Length R_wheel=0.31 "Dynamic rolling radius";
      parameter Units.Acceleration g=9.81;
      parameter Units.Velocity v_start=0 "Initial speed";
      Rotational.Interfaces.Flange_a axle "Wheel axle (torque at the wheels)"
        annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
      Daedalus.Blocks.Interfaces.RealOutput v(unit="m/s", start=v_start, fixed=true) "Vehicle speed"
        annotation (Placement(transformation(extent={{100,50},{120,70}})));
      Daedalus.Blocks.Interfaces.RealOutput s(unit="m", start=0, fixed=true) "Travelled distance"
        annotation (Placement(transformation(extent={{100,-70},{120,-50}})));
      Units.Acceleration a "Longitudinal acceleration";
      Units.Force F_tractive "Tractive force at the contact patch";
      Units.Force F_resistance "Sum of driving resistances";
      Units.Power P_road "Power to overcome driving resistances";
    protected
      parameter Units.Mass m_eff=m + J_wheels/R_wheel^2;
    equation
      der(s) = v;
      a = der(v);
      axle.phi = s/R_wheel;
      F_tractive = axle.tau/R_wheel;
      m_eff*a = F_tractive - F_resistance;
      P_road = F_resistance*v;
      annotation (Icon(graphics={
          Polygon(points={{-90,-20},{-90,20},{-50,30},{-20,60},{50,60},{80,20},{90,15},{90,-20},{-90,-20}},
            lineColor={30,60,110}, fillColor={200,215,235}, fillPattern=FillPattern.Solid),
          Ellipse(extent={{-70,-5},{-30,-45}}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
          Ellipse(extent={{40,-5},{80,-45}}, fillColor={60,60,60}, fillPattern=FillPattern.Solid),
          Text(extent={{-150,110},{150,70}}, textString="%name", textColor={0,0,255})}));
    end PartialVehicle;
  end Interfaces;

  package Chassis "Vehicle bodies"
    extends Daedalus.Icons.Package;

    model VehicleBody "Vehicle with aerodynamic drag (incl. crosswind yaw), rolling resistance and road grade"
      extends Interfaces.PartialVehicle;
      parameter Real Cd=0.29 "Drag coefficient at zero yaw";
      parameter Units.Area A_front=2.3 "Frontal area";
      parameter Real k_yaw(unit="1/rad2")=1.5 "Drag increase with yaw: Cd(beta) = Cd*(1 + k_yaw*beta^2)";
      parameter Real crr=0.010 "Rolling resistance coefficient";
      parameter Units.Density rho_air=1.2;
      parameter Real grade=0 "Road grade (rise/run), e.g. 0.02 for 2 %";
      parameter Units.Velocity v_wind=0 "Wind speed";
      parameter Units.Angle psi_wind=0 "Wind direction relative to the direction of travel (0: head wind, pi/2: pure crosswind)";
      Units.Velocity v_air_x "Air speed along the vehicle axis";
      Units.Velocity v_air_y "Lateral air speed";
      Units.Angle beta "Aerodynamic yaw angle";
      Units.Force F_aero, F_roll, F_grade;
    protected
      parameter Units.Angle theta=atan(grade);
    equation
      v_air_x = v + v_wind*cos(psi_wind);
      v_air_y = v_wind*sin(psi_wind);
      beta = atan2(v_air_y, max(abs(v_air_x), 0.1));
      F_aero = 0.5*rho_air*Cd*(1 + k_yaw*beta^2)*A_front*v_air_x*sqrt(v_air_x^2 + v_air_y^2);
      F_roll = crr*m*g*cos(theta)*tanh(v/0.1);
      F_grade = m*g*sin(theta);
      F_resistance = F_aero + F_roll + F_grade;
    end VehicleBody;

    model RoadLoadVehicle "Vehicle with coast-down road load F = F0 + F1*v + F2*v^2 (dynamometer ABC coefficients)"
      extends Interfaces.PartialVehicle;
      parameter Units.Force F0=150 "Constant term (A)";
      parameter Real F1(unit="N.s/m")=1 "Linear term (B)";
      parameter Real F2(unit="N.s2/m2")=0.4 "Quadratic term (C)";
    equation
      F_resistance = (F0 + F1*abs(v))*tanh(v/0.1) + F2*v*abs(v);
    end RoadLoadVehicle;
  end Chassis;

  package Powertrain "Electric and combustion powertrain components"
    extends Daedalus.Icons.Package;

    model ElectricMachine "Torque-controlled traction machine with torque and power limits and a loss map"
      parameter Units.Torque tau_max=350 "Peak torque";
      parameter Units.Power P_max=150e3 "Peak power (motoring and generating)";
      parameter Real k_cu(unit="W/(N.m)2")=0.04 "Copper loss coefficient: k_cu*tau^2";
      parameter Real k_fe(unit="W.s2/rad2")=0.002 "Iron/windage loss coefficient: k_fe*w^2";
      parameter Units.Power P_0=150 "Constant losses when energised (inverter, cooling)";
      parameter Units.Inertia J_rotor=0.05;
      Rotational.Interfaces.Flange_b flange "Shaft" annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Daedalus.Blocks.Interfaces.RealInput tau_ref(unit="N.m") "Torque request"
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Daedalus.Blocks.Interfaces.RealOutput P_el(unit="W") "Electrical power drawn (negative when regenerating)"
        annotation (Placement(transformation(origin={0,-110}, extent={{-10,-10},{10,10}}, rotation=270)));
      Units.AngularVelocity w(start=0) "Shaft speed";
      Units.Torque tau "Delivered air-gap torque";
      Units.Torque tau_avail "Available torque at the current speed";
      Units.Power P_mech, P_loss;
    equation
      w = der(flange.phi);
      tau_avail = min(tau_max, P_max/max(abs(w), 1));
      tau = min(tau_avail, max(-tau_avail, tau_ref));
      J_rotor*der(w) = tau + flange.tau;
      P_mech = tau*w;
      P_loss = P_0 + k_cu*tau^2 + k_fe*w^2;
      P_el = P_mech + P_loss;
      annotation (Icon(graphics={
          Rectangle(extent={{-80,50},{60,-50}}, lineColor={0,0,0}, fillColor={0,128,255}, fillPattern=FillPattern.HorizontalCylinder),
          Rectangle(extent={{60,10},{100,-10}}, fillColor={128,128,128}, fillPattern=FillPattern.HorizontalCylinder),
          Text(extent={{-150,100},{150,60}}, textString="%name", textColor={0,0,255})}));
    end ElectricMachine;

    model Battery "Battery pack: SOC-dependent open-circuit voltage and internal resistance, driven by power"
      parameter Real capacity(unit="A.h")=200 "Capacity (e.g. 96s2p cells)";
      parameter Units.Voltage V_empty=320 "Open-circuit voltage at SOC = 0";
      parameter Units.Voltage V_full=400 "Open-circuit voltage at SOC = 1";
      parameter Units.Resistance R_int=0.08;
      parameter Real SOC_start=0.9;
      Daedalus.Blocks.Interfaces.RealInput P(unit="W") "Power drawn at the terminals"
        annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Daedalus.Blocks.Interfaces.RealOutput SOC(start=SOC_start, fixed=true)
        annotation (Placement(transformation(extent={{100,-10},{120,10}})));
      Units.Voltage V_oc, V;
      Units.Current I;
      Units.Power P_loss;
      Real E_out(unit="J", start=0, fixed=true) "Energy delivered at the terminals";
      Real E_kWh(unit="kW.h") "Energy delivered in kWh";
    equation
      V_oc = V_empty + (V_full - V_empty)*SOC;
      I = (V_oc - sqrt(max(V_oc^2 - 4*R_int*P, 1)))/(2*R_int);
      V = V_oc - R_int*I;
      P_loss = R_int*I^2;
      der(SOC) = -I/(3600*capacity);
      der(E_out) = P;
      E_kWh = E_out/3.6e6;
      annotation (Icon(graphics={Rectangle(extent={{-80,50},{80,-50}}, lineColor={0,0,0}, fillColor={240,240,240}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{80,20},{95,-20}}, fillColor={0,0,0}, fillPattern=FillPattern.Solid),
          Rectangle(extent={{-74,44},{20,-44}}, lineColor={60,170,90}, fillColor={60,170,90}, fillPattern=FillPattern.Solid),
          Text(extent={{-150,100},{150,60}}, textString="%name", textColor={0,0,255})}));
    end Battery;

    model Engine "Combustion engine: full-load torque curve scaled by throttle, with drag torque and inertia"
      parameter Real torqueCurve[:, 2]=[800, 160; 1500, 210; 2500, 250; 3500, 250; 4500, 235; 5500, 210; 6300, 180; 6800, 0]
        "Full-load torque [rpm, N.m]";
      parameter Units.Torque tau_drag=20 "Friction (motoring) torque at 3000 rpm";
      parameter Units.Inertia J=0.2 "Engine (crank + flywheel) inertia";
      parameter Units.AngularVelocity w_start=800*2*3.141592653589793/60;
      Rotational.Interfaces.Flange_b flange annotation (Placement(transformation(extent={{90,-10},{110,10}})));
      Daedalus.Blocks.Interfaces.RealInput throttle "Throttle 0..1" annotation (Placement(transformation(extent={{-140,-20},{-100,20}})));
      Units.AngularVelocity w(start=w_start, fixed=true);
      Real rpm(unit="rev/min");
      Units.Torque tau_full, tau_eng;
      Units.Power P;
    equation
      rpm = w*60/(2*3.141592653589793);
      tau_full = Daedalus.Blocks.Functions.interpolate(torqueCurve, rpm, 2);
      tau_eng = min(1, max(0, throttle))*tau_full - tau_drag*(rpm/3000)*tanh(rpm/100);
      J*der(w) = tau_eng + flange.tau;
      w = der(flange.phi);
      P = tau_eng*w;
      annotation (Icon(graphics={Rectangle(extent={{-80,60},{60,-60}}, lineColor={0,0,0}, fillColor={210,80,60}, fillPattern=FillPattern.Solid, radius=8),
          Rectangle(extent={{60,10},{100,-10}}, fillColor={128,128,128}, fillPattern=FillPattern.HorizontalCylinder),
          Text(extent={{-150,110},{150,70}}, textString="%name", textColor={0,0,255})}));
    end Engine;
  end Powertrain;

  package Drivers "Driver models"
    extends Daedalus.Icons.Package;

    block Driver "PI speed-following driver with acceleration feed-forward; output is a normalised pedal (-1 brake .. 1 accelerate)"
      extends Daedalus.Icons.Block;
      parameter Real k=2 "Proportional gain [1/(m/s)]";
      parameter Units.Time Ti=5 "Integral time";
      parameter Units.Time T_preview=1 "Feed-forward preview time";
      parameter Real k_ff=0.4 "Feed-forward gain [1/(m/s2)]";
      Daedalus.Blocks.Interfaces.RealInput v_ref(unit="m/s") annotation (Placement(transformation(extent={{-140,40},{-100,80}})));
      Daedalus.Blocks.Interfaces.RealInput v(unit="m/s") annotation (Placement(transformation(extent={{-140,-80},{-100,-40}})));
      Daedalus.Blocks.Interfaces.RealOutput pedal annotation (Placement(transformation(extent={{100,-10},{120,10}})));
      Real e "Speed error";
      Real xi(start=0, fixed=true) "Integral of the error";
      Real v_ref_f(start=0, fixed=true) "Filtered reference used for the feed-forward";
      Real pedal_u "Unlimited pedal";
    equation
      e = v_ref - v;
      der(v_ref_f) = (v_ref - v_ref_f)/T_preview;
      pedal_u = k*(e + xi/Ti) + k_ff*der(v_ref_f);
      pedal = min(1, max(-1, pedal_u));
      der(xi) = if (pedal_u > 1 and e > 0) or (pedal_u < -1 and e < 0) then 0 else e;
      annotation (Icon(graphics={Ellipse(extent={{-30,80},{30,20}}, fillColor={255,220,180}, fillPattern=FillPattern.Solid),
          Polygon(points={{-50,10},{50,10},{40,-70},{-40,-70},{-50,10}}, fillColor={60,90,160}, fillPattern=FillPattern.Solid)}));
    end Driver;
  end Drivers;

  package Sensors "Energy accounting"
    extends Daedalus.Icons.Package;

    block EnergyMeter "Integrates power and normalises it by distance"
      extends Daedalus.Icons.Block;
      Daedalus.Blocks.Interfaces.RealInput P(unit="W") annotation (Placement(transformation(extent={{-140,40},{-100,80}})));
      Daedalus.Blocks.Interfaces.RealInput s(unit="m") annotation (Placement(transformation(extent={{-140,-80},{-100,-40}})));
      Real E(unit="J", start=0, fixed=true) "Energy";
      Real E_kWh(unit="kW.h");
      Real Wh_per_km(unit="W.h/km") "Specific consumption";
    equation
      der(E) = P;
      E_kWh = E/3.6e6;
      Wh_per_km = E/3600/max(s/1000, 0.01);
    end EnergyMeter;
  end Sensors;
end Vehicle;
