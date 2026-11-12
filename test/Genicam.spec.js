// The regression tests of the Java version (GenicamServiceTests.groovy) on MR-Moqui: the mock camera FLIR_CAMERA_1
// of GenicamTestData.xml, which the Python bridge simulates (no camera, SDK or Harvesters needed; NumPy and OpenCV
// are). See test/README.md.
spec({
    setup() { ec.artifactExecution.disableAuthz(); },
    cleanup() { ec.artifactExecution.enableAuthz(); },
});

const parameter = id => ec.entity.find("moqui.math.Parameter").condition("parameterId", id).useCache(false).one();
// The bridge answers a few paths relative to its working directory: the directory of the runtime directory when that is
// named runtime, else the runtime directory (a test's scratch runtime).
function absolute(location) {
    if (location.startsWith("/") || /^[A-Za-z]:/.test(location)) return location;
    const runtime = String(ec.factory.getRuntimePath()).replace(/\\/g, "/").replace(/\/$/, "");
    return runtime.endsWith("/runtime") && location.startsWith("runtime/")
        ? runtime + location.substring(7) : runtime + "/" + location;
}
const exists = location => ec.resource.getLocationReference("file:" + absolute(location)).getExists();
const size = location => ec.resource.getLocationReference("file:" + location).getSize();

feature("a read request stores the camera's values through store#Samples", {
    when() {
        const result = call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_ReadState"});
        return {result, exposure:parameter("11001"), gain:parameter("11002"), mode:parameter("11003"),
            source:parameter("11004")};
    },
    then(out) {
        out.result.status == "success";
        out.result.samples.length == 4;
        out.exposure.numericValue == 5000;
        out.gain.numericValue == 0;
        out.mode.symbolicValue == "On";
        out.source.symbolicValue == "Software";
    },
});

feature("a write request sends the software trigger and stores the camera's answer", {
    when() {
        const result = call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_TriggerShot"});
        return {result, trigger:parameter("11005")};
    },
    then(out) {
        out.result.status == "success";
        out.trigger.symbolicValue == "Executed";
    },
});

feature("the latest frame of the streaming thread is read as a file path", {
    when() {
        call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_StartStreaming"});
        const result = call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_GetFrame"});
        const location = parameter("11009").symbolicValue;
        call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_StopStreaming"});
        return {result, location};
    },
    then(out) {
        out.result.status == "success";
        out.location.endsWith(".jpg");
        exists(out.location);
    },
});

const isJpeg = bytes => bytes != null && bytes.length > 2 && bytes[0] == 0xFF && bytes[1] == 0xD8;

feature("the live view gets the latest frame of the streaming thread as JPEG", {
    when() {
        const live = () => call("moqui.genicam.GenicamServices.get#LiveFrame", {deviceId:"FLIR_CAMERA_1"});
        const stopped = live();
        call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_StartStreaming"});
        // the mock streaming thread caches a frame every genicam.stream.mock.frame.delay.ms
        let streaming = live();
        const deadline = Date.now() + 5000;
        while (streaming.source != "stream" && Date.now() < deadline) streaming = live();
        call("moqui.device.DeviceServices.run#DeviceRequest", {requestName:"FLIR_StopStreaming"});
        return {stopped, streaming};
    },
    then(out) {
        // the placeholder before any streaming; the last cached frame after a stream stopped (an earlier feature)
        ["fallback", "stream"].includes(out.stopped.source);
        isJpeg(out.stopped.jpegBytes);
        out.streaming.source == "stream";
        isJpeg(out.streaming.jpegBytes);
        out.streaming.capturedAt != null;
    },
});

feature("a single image is saved and registered as DeviceContent", {
    when() {
        const output = call("moqui.genicam.GenicamServices.acquire#SingleImage", {deviceId:"FLIR_CAMERA_1"});
        return {output, content:ec.entity.find("moqui.device.DeviceContent")
            .condition("deviceContentId", output.deviceContentId).one()};
    },
    then(out) {
        out.output.imageFormat == "jpg";
        out.output.contentType == "image/jpeg";
        out.output.width == 640;
        out.output.height == 480;
        size(out.output.imageLocation) > 0;
        out.content.deviceId == "FLIR_CAMERA_1";
        out.content.contentTypeEnumId == "DCntImage";
        out.content.contentLocation == out.output.imageLocation;
    },
});

feature("a video stream registers each frame as DeviceContent", {
    when() {
        return call("moqui.genicam.GenicamServices.acquire#VideoStream", {deviceId:"FLIR_CAMERA_1", numFrames:3});
    },
    then(out) {
        out.acquiredFrames.length == 3;
        out.deviceContentIdList.length == 3;
    },
});

feature("a video file is recorded", {
    when() {
        return call("moqui.genicam.GenicamServices.acquire#VideoFile", {deviceId:"FLIR_CAMERA_1", numFrames:10, fps:10});
    },
    then(out) {
        out.acquiredFrames == 10;
        Number(String(out.fps)) == 10;
        size(out.videoLocation) > 0;
    },
});

feature("a multi-camera recording runs a worker process per camera", {
    when() {
        return call("moqui.genicam.GenicamServices.acquire#MultiVideoFile", {
            deviceIdList:["FLIR_CAMERA_1", "FLIR_CAMERA_1"], numFrames:4, fps:5});
    },
    then(out) {
        out.captureResults.length == 2;
        out.successfulCount == 2;
        out.failedCount == 0;
        out.captureResults.every(item => item.success && exists(item.result.video_path));
    },
});

feature("a visual servo frame returns its bytes, capture time and registered snapshot", {
    when() {
        const output = call("moqui.genicam.GenicamServices.acquire#VisualServoFrame", {
            deviceId:"FLIR_CAMERA_1", useCachedFrame:false, saveSnapshot:true});
        return {output, content:ec.entity.find("moqui.device.DeviceContent")
            .condition("deviceContentId", output.snapshotDeviceContentId).one()};
    },
    then(out) {
        out.output.frameBytes.length > 0;
        out.output.contentType == "image/jpeg";
        out.output.dataFormat == "Mono8";
        out.output.frameFormat == "jpg";
        out.output.width == 640;
        out.output.height == 480;
        out.output.source == "capture";
        Number(String(out.output.capturedAt)) > 0;
        out.content.contentLocation == out.output.snapshotLocation;
    },
});

feature("a 3D frame is stored as a Tensor with its axes and NPY content", {
    when() {
        const output = call("moqui.genicam.GenicamServices.acquire#GenICam3DFrame", {deviceId:"FLIR_CAMERA_1"});
        return {output, tensor:ec.entity.find("moqui.math.Tensor").condition("tensorId", output.tensorId).one(),
            axes:ec.entity.find("moqui.math.TensorAxis").condition("tensorId", output.tensorId).count()};
    },
    then(out) {
        out.tensor.tensorTypeEnumId == "TtDense";
        out.tensor.purposeEnumId == "TpImageRep";
        out.tensor.rank == 3;
        out.tensor.shape == "[480, 640, 3]";
        out.tensor.size == 480 * 640 * 3;
        out.axes == 3;
        size(out.output.contentLocation) > 0;
    },
});

feature("tensor retention removes the GenICam tensors and keeps the others", {
    when() {
        const owned = call("moqui.genicam.GenicamServices.acquire#GenICam3DFrame", {deviceId:"FLIR_CAMERA_1"});
        const foreign = call("create#moqui.math.Tensor", {tensorTypeEnumId:"TtDense", rank:1, shape:"[3]", size:3,
            name:"Unrelated tensor for retention regression"});
        const foreignPath = owned.contentLocation + ".foreign.npy";
        ec.resource.getLocationReference("file:" + foreignPath).putBytes(new Uint8Array([1, 2, 3]));
        const foreignContent = call("create#moqui.math.TensorContent", {tensorId:foreign.tensorId,
            contentTypeEnumId:"TCntNpy", contentLocation:foreignPath, contentDate:"2000-01-01 00:00:00"});
        const result = call("moqui.genicam.GenicamServices.clean#GenICamTensors", {daysToKeep:0});
        const output = {result, ownedFile:exists(owned.contentLocation), foreignFile:exists(foreignPath),
            ownedTensor:ec.entity.find("moqui.math.Tensor").condition("tensorId", owned.tensorId).one(),
            ownedAxes:ec.entity.find("moqui.math.TensorAxis").condition("tensorId", owned.tensorId).count(),
            foreignContent:ec.entity.find("moqui.math.TensorContent")
                .condition("tensorContentId", foreignContent.tensorContentId).one()};
        call("moqui.genicam.GenicamServices.delete#GenICamTensor", {tensorId:foreign.tensorId});
        ec.resource.getLocationReference("file:" + foreignPath).delete();
        return output;
    },
    then(out) {
        out.result.deletedCount >= 1;
        out.ownedFile == false;
        out.ownedTensor == null;
        out.ownedAxes == 0;
        out.foreignFile == true;
        out.foreignContent != null;
    },
});

feature("a camera cannot use the connection of another device", {
    allowMessageErrors:true,
    when() {
        let failed = false;
        try {
            call("moqui.genicam.GenicamServices.acquire#SingleImage", {deviceId:"FLIR_CAMERA_1",
                connectionName:"FlirCameraConnectionReal"});
            failed = ec.message.hasError();
        } catch (error) { failed = true; }
        ec.message.clearErrors();
        return {failed};
    },
    then(out) { out.failed == true; },
});

// GenicamRealCameraTests.groovy: a connected FLIR camera, only with genicam_real_enabled=true (and the Spinnaker
// GenTL producer in FlirCameraConnectionReal, or genicam_real_cti_path).
feature("a real FLIR camera captures an image and a video file", {
    ignore: System.getProperty("genicam_real_enabled") !== "true",
    when() {
        const ctiPath = System.getProperty("genicam_real_cti_path");
        if (ctiPath) call("update#moqui.device.DeviceConnection", {connectionName:"FlirCameraConnectionReal", transportConfig:ctiPath});
        const parameters = {deviceId:"FLIR_CAMERA_REAL_19176722", connectionName:"FlirCameraConnectionReal",
            outputDir:"runtime/genicam/manual-tests/moqui-real"};
        return {image:call("moqui.genicam.GenicamServices.acquire#SingleImage", parameters),
            video:call("moqui.genicam.GenicamServices.acquire#VideoFile", {...parameters, numFrames:30, fps:10})};
    },
    then(out) {
        size(out.image.imageLocation) > 0;
        size(out.video.videoLocation) > 0;
    },
});
