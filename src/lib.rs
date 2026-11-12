/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Native code of moqui-genicam: the Rust port of the Groovy classes of the Java version. The services stay XML
//! Actions (GenicamServices.xml) and the logic of their Groovy scripts is the JavaScript library Genicam.js; the
//! classes that talked to the camera are tools:
//!
//! - `GenicamClient` (client.rs): `org.moqui.genicam.GenicamClient`, the camera through `script/genicam_bridge.py`,
//!   in a Python worker process (worker.rs) in place of the JEP interpreter.
//! - `GenicamMultiCameraRunner` (multi_camera.rs): `org.moqui.genicam.GenicamMultiCameraRunner`, the
//!   `script/multi_camera_capture.py` process of a multi-camera recording.
//!
//! The Python scripts are the camera side: Harvesters and the vendor's GenTL producer (Teledyne FLIR Spinnaker) are
//! Python, and they are kept as they are.

mod client;
mod multi_camera;
mod worker;

use std::sync::Arc;

use moqui_core::context::Error;
use moqui_core::native::{submit_component, NativeComponent, NativeRegistry};

submit_component! { NativeComponent { name: "moqui-genicam", register } }

fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool("moqui_genicam::GenicamClientFactory", || {
        Arc::new(client::GenicamClientFactory::default())
    })?;
    registry.tool("moqui_genicam::GenicamMultiCameraRunnerFactory", || {
        Arc::new(multi_camera::GenicamMultiCameraRunnerFactory)
    })
}
