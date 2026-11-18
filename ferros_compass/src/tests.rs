//! Host tests: the model against NOAA's published test values, the heading against synthetic devices at known tilts, and the hard-iron fit against a synthetic offset.

extern crate std;
use super::*;
use core::f64::consts::{PI, TAU};
use std::println;

const DEG: f64 = PI / 180.;

/// The official WMM2025 test values, `WMM2025_TestValues.txt` in `WMM2025COF.zip` from NOAA NCEI (<https://www.ncei.noaa.gov/products/world-magnetic-model/wmm-coefficients>), the same rows as the WMM2025 report's test table.
/// Columns as published: decimal year, height above the WGS84 ellipsoid (km), geodetic latitude, longitude (degrees), D, I (degrees), H, X, Y, Z, F (nT).
/// The rate columns are left out: this engine does not compute rates.
const TEST_VALUES: [[f64; 11]; 100] = [
    [2025., 28., 89., -121., -99.77, 88.47, 1504.298146, -255.388723, -1482.460628, 56194.288771, 56214.419888],
    [2025., 48., 80., -96., -29.91, 87.77, 2164.285547, 1875.982280, -1079.269389, 55623.044051, 55665.134163],
    [2025., 54., 82., 87., 54.89, 87.68, 2302.427342, 1324.336929, 1883.428620, 56740.772059, 56787.466800],
    [2025., 65., 43., 93., 0.50, 64.10, 24300.764692, 24299.852822, 210.517066, 50037.923998, 55626.621348],
    [2025., 51., -33., 109., -5.49, -67.50, 21838.046477, 21737.778822, -2090.274098, -52710.003920, 57054.752538],
    [2025., 39., -59., -8., -15.75, -58.55, 14918.115796, 14358.095523, -4049.107540, -24389.086374, 28589.818346],
    [2025., 3., -50., -103., 27.96, -54.89, 22106.041373, 19526.532799, 10362.990980, -31437.562789, 38431.724126],
    [2025., 94., -29., -110., 15.74, -38.25, 24181.990098, 23275.471080, 6559.046509, -19063.605287, 30792.688931],
    [2025., 66., 14., 143., -0.19, 12.82, 35003.635649, 35003.441364, -116.624818, 7966.315182, 35898.700342],
    [2025., 18., 0., 21., 1.29, -26.06, 29282.246275, 29274.811882, 659.800118, -14316.722540, 32594.761714],
    [2025.5, 6., -36., -137., 20.28, -52.11, 25353.230658, 23781.930678, 8786.698927, -32577.518648, 41280.516301],
    [2025.5, 63., 26., 81., 0.51, 41.07, 34803.980117, 34802.613433, 308.431881, 30332.056989, 46166.554053],
    [2025.5, 69., 38., -144., 12.93, 56.97, 23096.337032, 22510.804524, 5167.636206, 35525.990264, 42373.774537],
    [2025.5, 50., -70., -133., 57.21, -71.94, 16656.709403, 9021.847823, 14001.865232, -51084.838301, 53731.803175],
    [2025.5, 8., -52., -75., 14.91, -49.63, 20005.506364, 19331.857134, 5147.774727, -23532.664273, 30886.996822],
    [2025.5, 8., -66., 17., -33.14, -59.55, 18154.870136, 15201.438652, -9925.501123, -30881.123197, 35822.382382],
    [2025.5, 22., -37., 140., 9.28, -68.62, 21688.847590, 21404.761773, 3498.897430, -55397.587760, 59492.006517],
    [2025.5, 40., -12., -129., 10.76, -15.46, 29105.866326, 28594.451238, 5432.201489, -8052.525092, 30199.248583],
    [2025.5, 44., 33., -118., 11.10, 57.89, 23678.743422, 23235.835850, 4558.379362, 37727.715752, 44542.826874],
    [2025.5, 50., -81., -67., 28.13, -67.61, 18292.842720, 16132.682326, 8623.494405, -44412.283868, 48032.062762],
    [2026., 74., -57., 3., -22.51, -58.65, 14362.206593, 13268.119649, -5498.179626, -23576.062921, 27606.226129],
    [2026., 46., -24., -122., 14.01, -34.17, 26638.500090, 25846.118652, 6448.863284, -18080.399376, 32194.883578],
    [2026., 69., 23., 63., 1.17, 35.92, 34565.858527, 34558.605297, 708.078839, 25043.405885, 42684.549360],
    [2026., 33., -3., -147., 9.71, -2.12, 30957.666082, 30514.086409, 5221.840663, -1146.978999, 30978.906535],
    [2026., 47., -72., -22., -6.32, -61.16, 18392.516222, 18280.800055, -2024.105320, -33397.486160, 38127.112857],
    [2026., 62., -14., 99., -1.43, -44.70, 33448.275663, 33437.828630, -835.919473, -33100.922253, 47058.030120],
    [2026., 83., 86., -46., -30.61, 86.84, 3000.255301, 2582.099403, -1527.839829, 54279.308437, 54362.163830],
    [2026., 82., -64., 87., -81.74, -75.40, 13974.248411, 2007.074870, -13829.362571, -53663.514288, 55453.154865],
    [2026., 34., -19., 43., -14.98, -52.33, 20212.448819, 19525.694050, -5224.017527, -26182.862330, 33076.961273],
    [2026., 56., -81., 40., -59.77, -68.45, 17877.818542, 9000.536024, -15446.900890, -45267.890393, 48670.301997],
    [2026.5, 14., 0., 80., -3.10, -17.15, 39489.089663, 39431.415992, -2133.456168, -12188.838466, 41327.424134],
    [2026.5, 12., -82., -68., 29.79, -68.07, 18497.480257, 16052.819558, 9190.416754, -45940.146795, 49524.275496],
    [2026.5, 44., -46., -42., -11.36, -54.39, 14140.316272, 13863.176808, -2785.834358, -19744.304022, 24285.511845],
    [2026.5, 43., 17., 52., 1.19, 23.95, 36002.676553, 35994.907968, 747.876551, 15990.895993, 39394.180708],
    [2026.5, 64., 10., 78., -1.53, 7.53, 39441.702532, 39427.724480, -1049.971869, 5214.811025, 39784.948821],
    [2026.5, 12., 33., -145., 11.96, 52.51, 24672.289649, 24136.839639, 5112.225422, 32162.934508, 40536.110231],
    [2026.5, 12., -79., 115., -137.58, -77.37, 13023.480845, -9613.650418, -8785.714482, -58104.306533, 59545.961164],
    [2026.5, 14., -33., -114., 18.12, -44.10, 24613.183584, 23393.133584, 7653.110952, -23854.293436, 34275.882505],
    [2026.5, 19., 29., 66., 2.24, 46.04, 32749.959268, 32725.000691, 1278.343380, 33958.454002, 47177.711160],
    [2026.5, 86., -11., 167., 10.24, -31.60, 33105.255278, 32578.377044, 5882.794932, -20368.224196, 38869.300019],
    [2027., 37., -66., -5., -17.22, -59.04, 17159.836500, 16390.771268, -5079.626554, -28608.243575, 33360.029814],
    [2027., 67., 72., -115., 13.73, 84.84, 5026.868699, 4883.287833, 1192.857435, 55689.615237, 55916.032175],
    [2027., 44., 22., 174., 6.46, 31.89, 28867.487799, 28683.972510, 3249.857362, 17961.198575, 33999.066253],
    [2027., 54., 54., 178., 0.63, 65.46, 20617.099795, 20615.871559, 225.041793, 45149.631876, 49634.202547],
    [2027., 57., -43., 50., -48.27, -63.13, 16833.417225, 11203.748204, -12563.437494, -33221.366617, 37242.759503],
    [2027., 44., -43., -111., 24.31, -52.57, 22462.470699, 20471.522067, 9245.505620, -29347.556906, 36957.295441],
    [2027., 12., -63., 178., 57.87, -79.14, 11720.691727, 6233.774935, 9925.455386, -61075.730989, 62190.188377],
    [2027., 38., 27., -169., 8.48, 42.66, 26106.758229, 25821.061601, 3851.701313, 24058.365347, 35501.658671],
    [2027., 61., 59., -77., -16.48, 78.68, 10884.812802, 10437.775689, -3087.391846, 54397.713552, 55476.034370],
    [2027., 67., -47., -32., -13.52, -57.98, 12805.786103, 12450.832543, -2994.148743, -20475.298079, 24150.072239],
    [2027.5, 8., 62., 53., 19.39, 76.67, 12997.751893, 12260.425578, 4315.497527, 54849.810218, 56368.814385],
    [2027.5, 77., -68., -7., -16.19, -59.82, 17262.817301, 16578.099495, -4813.676173, -29680.383821, 34335.550744],
    [2027.5, 98., -5., 159., 7.79, -23.22, 33857.496691, 33544.960996, 4589.735713, -14525.780052, 36841.937629],
    [2027.5, 34., -29., -107., 15.64, -37.45, 24446.053109, 23540.396213, 6592.363667, -18723.097084, 30792.269761],
    [2027.5, 60., 27., 65., 1.85, 42.83, 33079.422542, 33062.159421, 1068.555172, 30667.425573, 45108.083389],
    [2027.5, 73., -72., 95., -102.64, -76.49, 13306.747292, -2912.418045, -12984.118939, -55399.217725, 56974.931751],
    [2027.5, 96., -46., -85., 17.93, -47.37, 19914.457641, 18947.658009, 6129.590452, -21631.434316, 29402.458634],
    [2027.5, 0., -13., -59., -17.49, -15.26, 22401.493010, 21365.849306, -6732.560620, -6112.313520, 23220.406233],
    [2027.5, 16., 66., -178., 0.37, 75.67, 13821.614337, 13821.326215, 89.244328, 54092.646409, 55830.559897],
    [2027.5, 72., -87., 38., -65.44, -70.97, 16661.696834, 6926.051596, -15153.941754, -48295.635435, 51088.947370],
    [2028., 49., 20., 167., 5.10, 26.82, 30251.262453, 30131.436134, 2689.876669, 15295.611788, 33898.298187],
    [2028., 71., 5., -13., -6.47, -17.66, 28323.200567, 28142.647915, -3192.970202, -9017.928693, 29724.177504],
    [2028., 95., 14., 65., -0.51, 17.44, 36933.940251, 36932.466767, -329.910612, 11601.180703, 38713.089985],
    [2028., 86., -85., -79., 41.09, -70.25, 16867.459172, 12713.115116, 11085.480728, -46988.258092, 49924.018042],
    [2028., 30., -36., -64., -4.65, -40.08, 17398.651081, 17341.333537, -1411.102609, -14639.967305, 22738.551012],
    [2028., 75., 79., 125., -18.59, 87.42, 2582.332595, 2447.595900, -823.235047, 57308.751756, 57366.902212],
    [2028., 21., 6., -32., -14.34, -8.70, 28453.070088, 27567.094260, -7045.034525, -4352.046687, 28783.980055],
    [2028., 1., -76., -75., 29.87, -65.23, 19597.635210, 16993.742946, 9761.147808, -42479.847250, 46782.525885],
    [2028., 45., -46., -41., -11.68, -54.96, 13897.562372, 13609.973828, -2812.623735, -19816.196211, 24203.798713],
    [2028., 11., -22., -21., -23.24, -57.67, 13528.270036, 12430.606447, -5337.987778, -21373.837958, 25295.356080],
    [2028.5, 28., 54., -120., 15.43, 73.74, 15286.094495, 14735.145798, 4066.959949, 52393.678137, 54578.037650],
    [2028.5, 68., -58., 156., 41.57, -81.52, 9282.943032, 6944.959164, 6159.591995, -62264.415929, 62952.605366],
    [2028.5, 39., -65., -88., 29.45, -60.20, 20609.270068, 17946.905687, 10131.662696, -35982.872979, 41466.964689],
    [2028.5, 27., -23., 81., -13.27, -58.58, 25625.329284, 24940.672257, -5883.907569, -41948.733657, 49156.421313],
    [2028.5, 11., 34., 0., 1.57, 46.77, 29089.194286, 29078.234597, 798.434048, 30945.577813, 42471.284539],
    [2028.5, 72., -62., 65., -67.87, -68.50, 17434.847799, 6567.891205, -16150.440331, -44267.327811, 47576.992646],
    [2028.5, 55., 86., 70., 67.64, 87.57, 2370.361097, 901.741754, 2192.139033, 55926.154052, 55976.363930],
    [2028.5, 59., 32., 163., 0.15, 43.10, 28217.204381, 28217.104175, 75.200106, 26405.862611, 38645.571587],
    [2028.5, 65., 48., 148., -9.55, 61.79, 23693.546804, 23365.167010, -3931.047029, 44177.553125, 50130.233993],
    [2028.5, 95., 30., 28., 4.56, 44.27, 29786.406936, 29692.133298, 2367.965026, 29039.808439, 41599.765773],
    [2029., 95., -60., -59., 8.58, -55.17, 18095.598879, 17893.018948, 2700.105868, -26011.845842, 31687.013474],
    [2029., 95., -70., 42., -55.06, -64.54, 18202.660480, 10426.249753, -14920.796381, -38237.047006, 42348.655378],
    [2029., 50., 87., -154., -73.48, 89.07, 906.920459, 257.877420, -869.484879, 55992.308183, 55999.652503],
    [2029., 58., 32., 19., 4.11, 46.03, 29434.989012, 29359.395781, 2108.188210, 30508.465542, 42393.219362],
    [2029., 57., 34., -13., -1.89, 45.74, 28257.277809, 28241.863242, -933.225495, 28997.458189, 40488.595068],
    [2029., 38., -76., 49., -64.28, -67.36, 18412.640681, 7991.121323, -16588.167977, -44151.303941, 47836.837024],
    [2029., 49., -50., -179., 32.11, -71.33, 18080.593790, 15315.042731, 9610.272521, -53522.651338, 56494.088877],
    [2029., 90., -55., -171., 38.65, -72.79, 16416.538468, 12821.507915, 10252.398258, -52995.143972, 55479.618058],
    [2029., 41., 42., -19., -4.13, 56.44, 24503.475397, 24439.977735, -1762.893879, 36929.897501, 44319.720622],
    [2029., 19., 46., -22., -5.65, 60.89, 22632.016516, 22522.142232, -2227.393290, 40651.688191, 46527.066578],
    [2029.5, 31., 13., -132., 9.04, 31.41, 28145.022897, 27795.620707, 4421.061344, 17187.983590, 32978.312476],
    [2029.5, 93., -2., 158., 7.09, -17.84, 34067.301020, 33806.520595, 4207.156292, -10964.647547, 35788.329028],
    [2029.5, 51., -76., 40., -56.34, -66.22, 18517.441118, 10263.650094, -15412.758102, -42018.541264, 45917.898858],
    [2029.5, 64., 22., -132., 10.23, 43.76, 26014.780783, 25601.266218, 4619.955329, 24914.378368, 36020.758857],
    [2029.5, 26., -65., 55., -63.48, -65.71, 18695.741849, 8347.198524, -16728.868464, -41431.528035, 45454.397791],
    [2029.5, 66., -21., 32., -14.63, -56.68, 16100.322907, 15578.335534, -4066.430831, -24494.877642, 29312.444941],
    [2029.5, 18., 9., -172., 9.24, 15.85, 30922.514410, 30520.999126, 4966.941695, 8779.472279, 32144.689001],
    [2029.5, 63., 88., 26., 36.52, 87.37, 2539.900024, 2041.140972, 1511.567287, 55286.620082, 55344.931586],
    [2029.5, 33., 17., 5., 0.89, 13.77, 34026.126581, 34021.976056, 531.446418, 8341.137769, 35033.582023],
    [2029.5, 77., -18., 138., 4.45, -47.55, 31847.600506, 31751.497580, 2472.257962, -34817.395113, 47186.021876],
];

fn wmm_row(row: &[f64; 11]) -> Field {
    field_at_year(row[2] * DEG, row[3] * DEG, row[1] * 1000., row[0])
}

#[test]
fn wmm2025_test_values() {
    let (mut worst_nt, mut worst_d, mut worst_i, mut worst_d_xy) = (0f64, 0f64, 0f64, 0f64);
    for row in TEST_VALUES.iter() {
        let b = wmm_row(row);
        for (got, want) in [(b.h, row[6]), (b.x, row[7]), (b.y, row[8]), (b.z, row[9]), (b.f, row[10])] {
            let err = (got - want).abs();
            if err > worst_nt {
                worst_nt = err;
            }
        }
        let ed = (b.d / DEG - row[4]).abs();
        let ei = (b.i / DEG - row[5]).abs();
        // The published D and I are rounded to 0.01 degree, so their own X and Y give a sharper check of D.
        let ed_xy = (b.d - atan2(row[8], row[7])).abs() / DEG;
        if ed > worst_d {
            worst_d = ed;
        }
        if ei > worst_i {
            worst_i = ei;
        }
        if ed_xy > worst_d_xy {
            worst_d_xy = ed_xy;
        }
    }
    println!("WMM2025 test values: worst nT {worst_nt:e}, D {worst_d:e} deg (vs X,Y {worst_d_xy:e}), I {worst_i:e} deg");
    // The published nT values carry six decimals; the model reproduces them to well under the 0.1 nT the report rounds its own table to.
    assert!(worst_nt < 1. / (1 << 10) as f64, "nT {worst_nt}");
    // Half the 0.01 degree rounding, plus a hair for the binary representation of the published decimal.
    assert!(worst_d < 0.005 + 1e-9, "D {worst_d}");
    assert!(worst_i < 0.005 + 1e-9, "I {worst_i}");
    // Against the six-decimal X and Y the only slack is their 1e-3 nT agreement over the weakest H (about 900 nT near a dip pole).
    assert!(worst_d_xy < 1e-5, "D vs X,Y {worst_d_xy}");
}

#[test]
fn eagle_time_to_decimal_year() {
    // The Eagle epoch itself.
    assert_eq!(eagle_from_civil(1969, 7, 20, (20 * 3600 + 17 * 60 + 40) as f64), 0.);
    // 2025 has 365 days: 182.5 days in is 2025-07-02 12:00.
    let t = eagle_from_civil(2025, 7, 2, 43_200.);
    assert!((decimal_year(t) - 2025.5).abs() < 1e-12);
    // 2028 is a leap year: 183 days in is 2028-07-02 00:00 and halfway.
    assert!((decimal_year(eagle_from_civil(2028, 7, 2, 0.)) - 2028.5).abs() < 1e-12);
    assert!((decimal_year(eagle_from_civil(2027, 1, 1, 0.)) - 2027.).abs() < 1e-12);
    // A test row reached thru Eagle time: 2027.0, 37 km, 66 S, 5 W.
    let row = TEST_VALUES.iter().find(|r| r[0] == 2027. && r[2] == -66. && r[3] == -5.).unwrap();
    let b = field(row[2] * DEG, row[3] * DEG, row[1] * 1000., eagle_from_civil(2027, 1, 1, 0.));
    assert!((b.z - row[9]).abs() < 1e-3);
    let d = declination(row[2] * DEG, row[3] * DEG, row[1] * 1000., eagle_from_civil(2027, 1, 1, 0.));
    assert!((d / DEG - row[4]).abs() < 0.005 + 1e-9);
}

#[test]
fn model_finite_at_the_pole() {
    let b = field_at_year(PI / 2., 0., 0., 2026.);
    assert!(b.x.is_finite() && b.y.is_finite() && b.z.is_finite());
    let b = field_at_year(-PI / 2., 1., 0., 2026.);
    assert!(b.x.is_finite() && b.y.is_finite() && b.z.is_finite());
}

// --- heading ---

fn rotate(v: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    // Rodrigues: v cos + (k x v) sin + k (k . v)(1 - cos), k a unit axis.
    let (c, s) = (cos(angle), sin(angle));
    let kxv = cross(axis, v);
    let kv = dot(axis, v);
    [
        v[0] * c + kxv[0] * s + axis[0] * kv * (1. - c),
        v[1] * c + kxv[1] * s + axis[1] * kv * (1. - c),
        v[2] * c + kxv[2] * s + axis[2] * kv * (1. - c),
    ]
}

/// A synthetic device: its x, y, z axes in the world frame (east, north, up).
struct Device {
    axes: [[f64; 3]; 3],
}

impl Device {
    /// Flat on a table, top edge toward azimuth `yaw` (clockwise from north).
    fn flat(yaw: f64) -> Device {
        Device { axes: [[cos(yaw), -sin(yaw), 0.], [sin(yaw), cos(yaw), 0.], [0., 0., 1.]] }
    }

    /// Tip the top edge up by `angle` (about the device's x axis).
    fn pitch(self, angle: f64) -> Device {
        let k = self.axes[0];
        Device { axes: [self.axes[0], rotate(self.axes[1], k, angle), rotate(self.axes[2], k, angle)] }
    }

    /// Roll about the device's y axis by `angle`.
    fn roll(self, angle: f64) -> Device {
        let k = self.axes[1];
        Device { axes: [rotate(self.axes[0], k, angle), self.axes[1], rotate(self.axes[2], k, angle)] }
    }

    /// A world vector as the device's sensors see it.
    fn sense(&self, world: [f64; 3]) -> [f32; 3] {
        [dot(world, self.axes[0]) as f32, dot(world, self.axes[1]) as f32, dot(world, self.axes[2]) as f32]
    }

    /// The true azimuth of a device-fixed world direction, measured from the world's north (here magnetic north).
    fn azimuth(dir: [f64; 3]) -> f64 {
        atan2(dir[0], dir[1]).rem_euclid(TAU)
    }
}

/// A field in east, north, up from the model's north, east, down, in microtesla.
fn enu_ut(b: &Field) -> [f64; 3] {
    [b.y / 1000., b.x / 1000., -b.z / 1000.]
}

fn angle_err(a: f64, b: f64) -> f64 {
    (a - b + PI).rem_euclid(TAU) - PI
}

#[test]
fn heading_at_known_tilts() {
    // Boulder, Colorado, early 2026: inclination about 65 degrees, a strong vertical part to cancel.
    let b = field_at_year(40. * DEG, -105. * DEG, 1600., 2026.);
    // Measure heading against magnetic north: put the horizontal field on the world's north axis.
    let world_field = [0., b.h / 1000., -b.z / 1000.];
    let up = [0., 0., 9.81];
    let mut worst = 0f64;
    let mut yaw = 0.;
    while yaw < TAU {
        for (pitch, roll) in [(0., 0.), (30. * DEG, 0.), (0., 45. * DEG), (30. * DEG, 45. * DEG), (-60. * DEG, -30. * DEG), (80. * DEG, 0.)] {
            let dev = Device::flat(yaw).pitch(pitch).roll(roll);
            let got = heading(dev.sense(world_field), dev.sense(up)).unwrap();
            let want = Device::azimuth(dev.axes[1]);
            let err = angle_err(got, want).abs();
            if err > worst {
                worst = err;
            }
        }
        yaw += TAU / (1 << 5) as f64;
    }
    println!("tilted heading worst error {:e} deg", worst / DEG);
    // The sensor vectors are f32, so the answer is good to f32 rounding.
    assert!(worst < 1e-5 * DEG * 100., "{worst}");
    // Pure pitch and roll leave the top edge's azimuth at the yaw.
    let dev = Device::flat(1.).pitch(30. * DEG);
    assert!(angle_err(heading(dev.sense(world_field), dev.sense(up)).unwrap(), 1.).abs() < 1e-5);
    let dev = Device::flat(1.).roll(45. * DEG);
    assert!(angle_err(heading(dev.sense(world_field), dev.sense(up)).unwrap(), 1.).abs() < 1e-5);
}

#[test]
fn upright_device_reports_the_back() {
    let b = field_at_year(40. * DEG, -105. * DEG, 1600., 2026.);
    let world_field = [0., b.h / 1000., -b.z / 1000.];
    let up = [0., 0., 9.81];
    let dev = Device::flat(2.).pitch(90. * DEG);
    // The top edge points at the sky: no heading, and it says so.
    assert_eq!(heading(dev.sense(world_field), dev.sense(up)), None);
    // The back faces the way the top edge pointed before it was stood up.
    let back = heading_back(dev.sense(world_field), dev.sense(up)).unwrap();
    assert!(angle_err(back, 2.).abs() < 1e-5, "{back}");
    // Flat on a table the back faces the ground: no back heading.
    let flat = Device::flat(2.);
    assert_eq!(heading_back(flat.sense(world_field), flat.sense(up)), None);
    // Just past the limit the top edge has a heading again.
    let leaning = Device::flat(2.).pitch(80. * DEG);
    assert!(heading(leaning.sense(world_field), leaning.sense(up)).is_some());
}

#[test]
fn heading_none_at_the_dip_pole_and_dead_sensor() {
    let up = [0., 0., 9.81];
    let dev = Device::flat(0.3);
    // A field straight down: no east.
    assert_eq!(heading(dev.sense([0., 0., -50.]), dev.sense(up)), None);
    // A magnetometer reading nothing.
    assert_eq!(heading([0.; 3], dev.sense(up)), None);
}

#[test]
fn true_heading_wraps() {
    let b = field_at_year(-36. * DEG, -137. * DEG, 6000., 2025.5);
    // Published declination there is 20.28 degrees east.
    let t = true_heading(350. * DEG, b.d);
    assert!(angle_err(t, (350. + 20.28) * DEG).abs() < 0.005 * DEG + 1e-9);
    assert!(t >= 0. && t < TAU);
    let t = true_heading(5. * DEG, -10. * DEG);
    assert!((t - 355. * DEG).abs() < 1e-12);
    // Full pipeline: a device at a known true azimuth reads it back thru the field in its true orientation.
    let world = enu_ut(&b);
    let up = [0., 0., 9.81];
    let dev = Device::flat(123. * DEG).pitch(20. * DEG).roll(-35. * DEG);
    let mag = heading(dev.sense(world), dev.sense(up)).unwrap();
    let truth = true_heading(mag, b.d);
    assert!(angle_err(truth, Device::azimuth(dev.axes[1])).abs() < 1e-5);
}

// --- hard iron ---

#[test]
fn hard_iron_full_turn() {
    let b = field_at_year(52. * DEG, 0., 0., 2026.);
    let f_ut = b.f / 1000.;
    let offset = [12.5f32, -30.25, 44.];
    let mut fit = HardIron::new();
    // Every orientation on a 15 degree grid, poles included, so each axis meets its extremes exactly.
    let step = 15. * DEG;
    let mut lat = -PI / 2.;
    while lat <= PI / 2. + 1e-9 {
        let mut lon = 0.;
        while lon < TAU {
            let u = [cos(lat) * cos(lon), cos(lat) * sin(lon), sin(lat)];
            fit.add([
                (f_ut * u[0]) as f32 + offset[0],
                (f_ut * u[1]) as f32 + offset[1],
                (f_ut * u[2]) as f32 + offset[2],
            ]);
            lon += step;
        }
        lat += step;
    }
    let cal = fit.calibration();
    let mut worst = 0f32;
    for a in 0..3 {
        let e = (cal.offset[a] - offset[a]).abs();
        if e > worst {
            worst = e;
        }
    }
    println!("hard iron full turn: worst offset error {worst:e} uT over {} samples", cal.samples);
    assert!(worst < 1e-4, "{worst}");
    assert_eq!(cal.samples, fit.samples());
    assert!(fit.is_good(f_ut as f32));
    let cov = fit.coverage(f_ut as f32);
    for c in cov {
        assert!((c - 1.).abs() < 1e-5, "{c}");
    }
    // Calibrated readings come back to the field's strength.
    let raw = [(f_ut * 0.6) as f32 + offset[0], (f_ut * 0.8) as f32 + offset[1], offset[2]];
    let m = cal.apply(raw);
    let len = sqrt((m[0] * m[0] + m[1] * m[1] + m[2] * m[2]) as f64);
    assert!((len - f_ut).abs() < 1e-4);
}

#[test]
fn hard_iron_flat_spin_is_not_enough() {
    // Spinning the device flat on a table sweeps x and y but never z: the puck must ask for more turning.
    let b = field_at_year(52. * DEG, 0., 0., 2026.);
    let f_ut = (b.f / 1000.) as f32;
    let world = enu_ut(&b);
    let offset = [5f32, 6., -7.];
    let mut fit = HardIron::new();
    let mut yaw = 0.;
    while yaw < TAU {
        let dev = Device::flat(yaw);
        let s = dev.sense(world);
        fit.add([s[0] + offset[0], s[1] + offset[1], s[2] + offset[2]]);
        yaw += TAU / (1 << 6) as f64;
    }
    let cov = fit.coverage(f_ut);
    println!("flat spin coverage {cov:?}");
    assert!(cov[2] < 1e-5);
    // x and y each swept twice the horizontal field: cos(inclination) of a full turn.
    let ci = (b.h / b.f) as f32;
    assert!((cov[0] - ci).abs() < 1e-3 && (cov[1] - ci).abs() < 1e-3);
    assert!(!fit.is_good(f_ut));
    // x and y offsets are already right; z is not knowable from a flat spin.
    let cal = fit.calibration();
    assert!((cal.offset[0] - offset[0]).abs() < 1e-2 && (cal.offset[1] - offset[1]).abs() < 1e-2);
}

#[test]
fn calibrated_heading_through_offset() {
    // A device with a strong hard-iron offset, calibrated by a full turn, then read at a tilt.
    let b = field_at_year(-33. * DEG, 151. * DEG, 50., 2026.5);
    let world = enu_ut(&b);
    let up = [0., 0., 9.81];
    let offset = [40f32, -25., 60.];
    let mut fit = HardIron::new();
    let mut yaw = 0.;
    while yaw < TAU {
        let mut tip = -PI;
        while tip < PI {
            for dev in [Device::flat(yaw).pitch(tip), Device::flat(yaw).roll(tip)] {
                let s = dev.sense(world);
                fit.add([s[0] + offset[0], s[1] + offset[1], s[2] + offset[2]]);
            }
            tip += TAU / (1 << 6) as f64;
        }
        yaw += TAU / (1 << 6) as f64;
    }
    assert!(fit.is_good((b.f / 1000.) as f32));
    let cal = fit.calibration();
    let dev = Device::flat(200. * DEG).pitch(25. * DEG).roll(15. * DEG);
    let s = dev.sense(world);
    let m = cal.apply([s[0] + offset[0], s[1] + offset[1], s[2] + offset[2]]);
    let got = true_heading(heading(m, dev.sense(up)).unwrap(), b.d);
    let err = angle_err(got, Device::azimuth(dev.axes[1])).abs() / DEG;
    println!("calibrated tilted heading error {err:e} deg, offset {:?}", cal.offset);
    assert!(err < 0.5, "{err}");
}
