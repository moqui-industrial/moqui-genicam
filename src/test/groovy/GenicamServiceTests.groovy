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
package org.moqui.genicam

import spock.lang.Shared
import spock.lang.Specification
import spock.lang.Stepwise
import org.moqui.Moqui
import org.moqui.context.ExecutionContext
import org.moqui.entity.EntityValue
import org.slf4j.Logger
import org.slf4j.LoggerFactory

@Stepwise
class GenicamServiceTests extends Specification {
    @Shared protected static final Logger logger = LoggerFactory.getLogger(GenicamServiceTests)
    @Shared ExecutionContext ec

    def setupSpec() {
        // ensure database is initialized and seed/test data is loaded
        Moqui.getExecutionContextFactory().checkEmptyDb()
        ec = Moqui.getExecutionContext()
        ec.user.loginUser("john.doe", "moqui")
    }

    def cleanupSpec() {
        if (ec != null) ec.destroy()
    }

    def setup() {
        ec.artifactExecution.disableAuthz()
    }

    def cleanup() {
        ec.artifactExecution.enableAuthz()
    }

    def "test read live camera state (mock fallback)"() {
        given: "the FLIR_ReadState request exists"
            EntityValue readReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_ReadState").one()
            assert readReq != null

        when: "executing the read request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_ReadState").call()

        then: "no errors are raised"
            !ec.message.hasError()

        and: "the parameters are correctly updated in the database"
            EntityValue exposure = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11001").one()
            EntityValue gain = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11002").one()
            EntityValue trigMode = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11003").one()
            EntityValue trigSrc = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11004").one()

            exposure.numericValue == 5000.0
            gain.numericValue == 0.0
            trigMode.symbolicValue == "On"
            trigSrc.symbolicValue == "Software"
            
        and: "ParameterLog entries are successfully generated"
            long logsCount = ec.entity.find("moqui.math.ParameterLog").condition("parameterId", "11001").count()
            logsCount > 0
    }

    def "test write software trigger parameter (mock fallback)"() {
        given: "the FLIR_TriggerShot request exists"
            EntityValue writeReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_TriggerShot").one()
            assert writeReq != null

        when: "triggering the software shot write request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_TriggerShot").call()

        then: "no errors are raised"
            !ec.message.hasError()

        and: "the trigger parameter log indicates execution"
            EntityValue triggerParam = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11005").one()
            triggerParam.symbolicValue == "Executed"
            
            long logEntries = ec.entity.find("moqui.math.ParameterLog").condition("parameterId", "11005").count()
            logEntries > 0
    }

    def "test start and stop camera streaming (mock fallback)"() {
        given: "the streaming requests exist"
            EntityValue startReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_StartStreaming").one()
            EntityValue stopReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_StopStreaming").one()
            assert startReq != null && stopReq != null

        when: "triggering the start streaming request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StartStreaming").call()

        then: "no errors are raised"
            !ec.message.hasError()

        when: "triggering the stop streaming request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StopStreaming").call()

        then: "no errors are raised"
            !ec.message.hasError()
    }

    def "test read latest frame (mock fallback)"() {
        given: "starting the background stream"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StartStreaming").call()
            assert !ec.message.hasError()

        when: "fetching the latest frame"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_GetFrame").call()

        then: "no errors are raised"
            !ec.message.hasError()

        and: "the LatestFrame parameter is updated with a valid file path"
            EntityValue latestFrameParam = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11009").one()
            String filePath = latestFrameParam.symbolicValue
            assert filePath != null && filePath.endsWith(".jpg")

        and: "the saved JPEG image exists on the filesystem and is non-empty"
            File imageFile = new File(filePath)
            assert imageFile.exists() && imageFile.length() > 0

        cleanup: "stopping the stream to clean up the acquisition thread"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StopStreaming").call()
    }

    def "test acquire 3D frame (mock fallback)"() {
        when: "calling the acquire#GenICam3DFrame service"
            Map res = ec.service.sync().name("moqui.genicam.GenicamServices.acquire#GenICam3DFrame")
                .parameter("deviceId", "FLIR_CAMERA_1").call()
            
        then: "no errors are raised"
            !ec.message.hasError()
            
        and: "a valid tensorId and tensorContentId are returned"
            res.tensorId != null
            res.tensorContentId != null
            res.contentLocation != null
            
        and: "the tensor record is created in the database with correct rank and shape"
            EntityValue tensor = ec.entity.find("moqui.math.Tensor").condition("tensorId", res.tensorId).one()
            tensor != null
            tensor.tensorTypeEnumId == "TtDense"
            tensor.purposeEnumId == "TpImageRep"
            tensor.rank == 3
            tensor.shape == "[480, 640, 3]"
            tensor.size == 480 * 640 * 3
            
        and: "the saved tensor .npy file exists on the filesystem and is non-empty"
            File npyFile = new File(res.contentLocation)
            npyFile.exists()
            npyFile.length() > 0
    }

    def "test clean tensors service job"() {
        given: "a tensor file and DB record exist"
            Map res = ec.service.sync().name("moqui.genicam.GenicamServices.acquire#GenICam3DFrame")
                .parameter("deviceId", "FLIR_CAMERA_1").call()
            assert !ec.message.hasError()
            
            String tensorId = res.tensorId
            String contentLocation = res.contentLocation
            assert new File(contentLocation).exists()
            
        when: "calling clean#GenICamTensors with daysToKeep=0 to force deletion"
            ec.service.sync().name("moqui.genicam.GenicamServices.clean#GenICamTensors")
                .parameter("daysToKeep", 0).call()
                
        then: "no errors are raised"
            !ec.message.hasError()
            
        and: "the tensor file is deleted from filesystem"
            !new File(contentLocation).exists()
            
        and: "the tensor DB records are deleted"
            ec.entity.find("moqui.math.Tensor").condition("tensorId", tensorId).one() == null
            ec.entity.find("moqui.math.TensorContent").condition("tensorId", tensorId).one() == null
            ec.entity.find("moqui.math.TensorAxis").condition("tensorId", tensorId).count() == 0
    }

    def "test device error status change on failure"() {
        given: "a device and connection with invalid transport config"
            EntityValue conn = ec.entity.find("moqui.device.DeviceConnection")
                .condition("connectionName", "FlirCameraConnection").one()
            assert conn != null
            String originalConfig = conn.transportConfig
            
            // Set device status to Standstill initially
            EntityValue device = ec.entity.find("moqui.device.Device").condition("deviceId", "FLIR_CAMERA_1").one()
            device = device.cloneValue()
            device.statusId = "DbsStandstill"
            device.update()
            
        when: "triggering acquire 3D frame with an invalid config path"
            conn = conn.cloneValue()
            conn.transportConfig = "invalid_path.cti"
            conn.update()
            
            try {
                ec.service.sync().name("moqui.genicam.GenicamServices.acquire#GenICam3DFrame")
                    .parameter("deviceId", "FLIR_CAMERA_1").call()
            } catch (Exception e) {
                // Expected to fail
            }
            
        then: "the device status in DB transitions to DbsErrorStop"
            EntityValue updatedDevice = ec.entity.find("moqui.device.Device")
                .condition("deviceId", "FLIR_CAMERA_1").one()
            updatedDevice.statusId == "DbsErrorStop"
            
        cleanup: "restore the original connection transport config"
            if (conn != null && originalConfig != null) {
                conn = conn.cloneValue()
                conn.transportConfig = originalConfig
                conn.update()
            }
    }
}
