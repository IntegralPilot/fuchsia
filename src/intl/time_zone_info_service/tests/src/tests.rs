// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use fable_lib::fable;
    use fidl_fuchsia_intl::{
        self as fintl, CivilTime, CivilToAbsoluteTimeOptions, DayOfWeek, Month,
        RepeatedTimeConversion, SkippedTimeConversion, TimeZoneId, TimeZoneInfo, TimeZonesProxy,
    };
    use fuchsia_component_test::{
        Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route,
    };

    static SVC_URL: &str =
        "fuchsia-pkg://fuchsia.com/time-zone-info-service-test#meta/time-zone-info-service.cm";

    static TZ_NYC: &str = "America/New_York";
    static NANOS_PER_SECOND: i64 = 1_000_000_000;
    static SECONDS_PER_HOUR: i64 = 3600;

    async fn connect_to_service() -> Result<(RealmInstance, TimeZonesProxy)> {
        const MONIKER: &str = "tzinfo";

        let builder = RealmBuilder::new().await?;
        let tzinfo = builder.add_child(MONIKER, SVC_URL, ChildOptions::new()).await?;
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol::<fintl::TimeZonesMarker>())
                    .from(&tzinfo)
                    .to(Ref::parent()),
            )
            .await?;

        let realm = builder.build().await?;
        let svc = realm.root.connect_to_protocol_at_exposed_dir()?;
        Ok((realm, svc))
    }

    #[fuchsia::test]
    async fn test_civil_to_absolute_time() -> Result<()> {
        let (realm, svc) = connect_to_service().await?;

        let civil_time = fable! {
          CivilTime {
              year: 2021,
              month: Month::August,
              day: 15,
              hour: 20,
              minute: 17,
              second: 42,
              nanos: 123_456_789,
              time_zone_id: TimeZoneId {
                  id: TZ_NYC.to_string(),
              },
          }
        };

        let options = fable! {
            CivilToAbsoluteTimeOptions {
                repeated_time_conversion: RepeatedTimeConversion::BeforeTransition,
                skipped_time_conversion: SkippedTimeConversion::NextValidTime,
            }
        };

        let actual = svc
            .civil_to_absolute_time(&civil_time, &options)
            .await?
            .map(zx::MonotonicInstant::from_nanos);
        realm.destroy().await?;

        let expected =
            Ok(zx::MonotonicInstant::from_nanos(1629073062 * NANOS_PER_SECOND + 123_456_789));
        assert_eq!(actual, expected);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_absolute_to_civil_time() -> Result<()> {
        let (realm, svc) = connect_to_service().await?;

        let absolute_time = 1629073062 * NANOS_PER_SECOND + 123_456_789;
        let tz_id = TimeZoneId { id: TZ_NYC.to_string() };

        let actual = svc.absolute_to_civil_time(&tz_id, absolute_time).await?;
        realm.destroy().await?;

        let expected = Ok(fable! {
          CivilTime {
              year: 2021,
              month: Month::August,
              day: 15,
              hour: 20,
              minute: 17,
              second: 42,
              nanos: 123_456_789,
              weekday: DayOfWeek::Sunday,
              year_day: 226,
              time_zone_id: TimeZoneId {
                  id: TZ_NYC.to_string(),
              },
          }
        });
        assert_eq!(actual, expected);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_time_zone_info() -> Result<()> {
        let (realm, svc) = connect_to_service().await?;

        let absolute_time = 1629073062 * NANOS_PER_SECOND + 123_456_789;
        let tz_id = TimeZoneId { id: TZ_NYC.to_string() };

        let actual = svc.get_time_zone_info(&tz_id, absolute_time).await?;
        realm.destroy().await?;

        let expected = Ok(fable! {
          TimeZoneInfo {
              id: tz_id,
              total_offset_at_time: -4 * SECONDS_PER_HOUR * NANOS_PER_SECOND,
              in_dst_at_time: true,
          }
        });
        assert_eq!(actual, expected);
        Ok(())
    }
}
