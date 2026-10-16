// Disposable database + GenicamTestData.xml; no physical camera required.
spec({setup() { ec.artifactExecution.disableAuthz(); }, cleanup() { ec.artifactExecution.enableAuthz(); }});

feature("visual servo returns binary buffers, capture time and persisted snapshot", {
    when() {
        const output = call("moqui.genicam.GenicamServices.acquire#VisualServoFrame", {
            deviceId:"FLIR_CAMERA_1", useCachedFrame:false, saveSnapshot:true,
        });
        return {output, content:ec.entity.find("moqui.device.DeviceContent")
            .condition("deviceContentId",output.snapshotDeviceContentId).one()};
    },
    then(out) {
        out.output.frameBytes.length > 0;
        out.output.jpegBytes.length > 0;
        Number(out.output.capturedAt) > 0;
        out.content.deviceId == "FLIR_CAMERA_1";
        out.content.contentLocation == out.output.snapshotLocation;
    },
});

feature("tensor retention removes only expired GenICam files and preserves foreign data", {
    when() {
        const owned = call("moqui.genicam.GenicamServices.acquire#GenICam3DFrame", {deviceId:"FLIR_CAMERA_1"});
        call("update#moqui.math.TensorContent", {tensorContentId:owned.tensorContentId, contentDate:"2000-01-01 00:00:00"});
        const foreign = call("create#moqui.math.Tensor", {tensorTypeEnumId:"TtDense", rank:1, shape:"[3]", size:3, name:"Unrelated tensor for retention regression"});
        const foreignPath = owned.contentLocation + ".foreign.npy";
        ec.resource.getLocationReference(foreignPath).putBytes(new Uint8Array([1,2,3]));
        const foreignContent = call("create#moqui.math.TensorContent", {tensorId:foreign.tensorId,
            contentTypeEnumId:"TCntNpy", contentLocation:foreignPath, contentDate:"2000-01-01 00:00:00"});
        const result = call("moqui.genicam.GenicamServices.clean#GenICamTensors", {daysToKeep:30});
        const output = {result, ownedFile:ec.resource.getLocationReference(owned.contentLocation).getExists(),
            foreignFile:ec.resource.getLocationReference(foreignPath).getExists(),
            foreignContent:ec.entity.find("moqui.math.TensorContent").condition("tensorContentId",foreignContent.tensorContentId).one()};
        call("delete#moqui.math.TensorContent", {tensorContentId:foreignContent.tensorContentId});
        call("delete#moqui.math.Tensor", {tensorId:foreign.tensorId});
        ec.resource.getLocationReference(foreignPath).delete();
        return output;
    },
    then(out) {
        out.result.deletedCount >= 1;
        out.ownedFile == false;
        out.foreignFile == true;
        out.foreignContent != null;
    },
});

feature("single capture creates a DeviceContent reference in Rust", {
    when() {
        return call("moqui.genicam.GenicamServices.acquire#SingleImage", {deviceId:"FLIR_CAMERA_1"});
    },
    then(out) { out.deviceContentId != null; out.imageLocation != null; },
});

feature("physical device serial is used when the connection has no serial override", {
    when() {
        call("update#moqui.device.DeviceConnection", {connectionName:"FlirCameraConnection", options:null});
        try {
            return call("moqui.genicam.GenicamServices.acquire#VisualServoFrame", {deviceId:"FLIR_CAMERA_1", useCachedFrame:false});
        } finally {
            call("update#moqui.device.DeviceConnection", {connectionName:"FlirCameraConnection", options:"serial=FLIR_CAMERA_1"});
        }
    },
    then(out) { out.frameBytes.length > 0; },
});

feature("three dimensional bridge buffers persist as Tensor and NPY content", {
    when() {
        const output = call("moqui.genicam.GenicamServices.acquire#GenICam3DFrame", {deviceId:"FLIR_CAMERA_1"});
        return {output, tensor:ec.entity.find("moqui.math.Tensor").condition("tensorId",output.tensorId).one()};
    },
    then(out) { out.output.tensorContentId != null; out.tensor.rank == 3; },
});

feature("a camera cannot select another device connection", {
    allowMessageErrors:true,
    when() {
        let failed = false;
        try {
            call("moqui.genicam.GenicamServices.acquire#VisualServoFrame", {
                deviceId:"FLIR_CAMERA_1", connectionName:"FlirCameraConnectionReal", useCachedFrame:false,
            });
            failed = ec.message.hasError();
        } catch (error) { failed = true; }
        return {failed};
    },
    then(out) { out.failed == true; },
});
