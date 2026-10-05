use vetra_types::sql::*;
#[test]
fn scalar_lossless_roundtrips_and_ordered_keys_agree() {
    let mut cases = vec![
        Scalar::Null,
        Scalar::Bool(false),
        Scalar::Bool(true),
        Scalar::Int(i64::MIN),
        Scalar::Int(-1),
        Scalar::Int(0),
        Scalar::Int(i64::MAX),
        Scalar::Float(f64::NEG_INFINITY.to_bits()),
        Scalar::Float((-0.0_f64).to_bits()),
        Scalar::Float(0.0_f64.to_bits()),
        Scalar::Float(f64::INFINITY.to_bits()),
        Scalar::Float(f64::NAN.to_bits()),
        Scalar::Text("a\0λ".into()),
        Scalar::Bytes(vec![0, 255]),
        Scalar::Uuid([255; 16]),
        Scalar::Date(-1),
        Scalar::Time(86399999999),
        Scalar::Timestamp(-1234567),
        Scalar::Timestamptz(1234567),
        Scalar::Json("{\"x\":9007199254740993}".into()),
    ];
    for n in [
        "-999999999999999999999999999999",
        "-100",
        "-1.1",
        "-1",
        "-0.001",
        "0",
        "0.001",
        "1",
        "1.1",
        "100",
        "999999999999999999999999999999",
    ] {
        cases.push(Scalar::decimal(n).unwrap())
    }
    for v in &cases {
        assert_eq!(Scalar::decode(&v.encode().unwrap()).unwrap(), *v)
    }
    for a in &cases {
        for b in &cases {
            if !matches!(a, Scalar::Json(_))
                && std::mem::discriminant(a) == std::mem::discriminant(b)
            {
                if let Some(order) = a.compare(b).unwrap() {
                    assert_eq!(
                        a.ordered().unwrap().cmp(&b.ordered().unwrap()),
                        order,
                        "{a:?} vs {b:?}"
                    )
                }
            }
        }
    }
    assert_eq!(
        Scalar::decimal("9007199254740993").unwrap().text(),
        "9007199254740993"
    );
}
#[test]
fn type_boundaries_null_precision_and_timezone_are_explicit() {
    assert_eq!(
        Scalar::Text("32768".into())
            .cast(&Type::Int2)
            .unwrap_err()
            .code,
        "22003"
    );
    assert_eq!(
        Scalar::Text("λx".into())
            .cast(&Type::Varchar(Some(1)))
            .unwrap_err()
            .code,
        "22001"
    );
    assert_eq!(
        Scalar::Text("2024-02-29".into())
            .cast(&Type::Date)
            .unwrap()
            .text(),
        "2024-02-29"
    );
    let a = Scalar::Text("2024-01-01T03:00:00+03:00".into())
        .cast(&Type::Timestamptz)
        .unwrap();
    let b = Scalar::Text("2024-01-01T00:00:00Z".into())
        .cast(&Type::Timestamptz)
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(
        Scalar::Text("2024-01-01 00:00:00".into())
            .cast(&Type::Timestamptz)
            .unwrap_err()
            .code,
        "22007"
    );
    assert_eq!(
        Scalar::Text("1.235".into())
            .cast(&Type::Numeric(Some((4, 2))))
            .unwrap()
            .text(),
        "1.24"
    );
    assert!(Scalar::decimal("1e100000").is_err());
    assert!(Scalar::decode(b"VSQLV001bad").is_err());
    for oid in [
        16, 17, 20, 21, 23, 25, 114, 700, 701, 1043, 1082, 1083, 1114, 1184, 1700, 2950, 3802,
    ] {
        assert_eq!(Type::from_oid(oid).unwrap().oid(), oid)
    }
}
